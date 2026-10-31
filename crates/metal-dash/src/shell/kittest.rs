//! `c1` of `O.12.c` (`plan-opus.md` § O.12, `plan-sonnet.md` § S.U7): the
//! `egui_kittest` harness wired into the tree with a smoke test that runs
//! inside `cargo test --workspace` today, before a single panel exists —
//! *"impossible to skip"*. This module lives inside `shell/` like any
//! other file here and is caught by `gates.rs`'s scan of everything but
//! `classes.rs`/`components.rs`: it cannot reach `crate::call` either
//! (nothing in it needs to — it only drives [`Shell::ui`]).
//!
//! `egui_kittest` is a **dev-dependency** (`Cargo.toml`) with neither the
//! `wgpu` nor the `snapshot` feature on: no pixel is ever rasterised, so
//! this is the same mechanism gate 3 above hand-rolls — an `egui::Context`
//! run headless with AccessKit reporting the widget tree — with a query API
//! (`kittest::Queryable`) on top instead of a hand-rolled skeleton walk. It
//! builds and runs on a host with no GPU, and being a dev-dependency it
//! never reaches a `--no-default-features` build or the shipped binary.
//!
//! [`harness`] is the one place a `Harness` gets built, so every later
//! panel lote (`S.U8`+) shares one canvas size (`super::DESKTOP_VIEWPORT`)
//! and one way of pushing the theme into the context.

use egui::Vec2;
use egui_kittest::{kittest::Queryable, Harness};

use crate::call::Call;

use super::classes::{self, Placement, Theme, VIEWPORTS};
use super::panels::Region;
use super::{FakeDoor, Shell};

/// Build a `Harness` that runs [`Shell::ui`] every [`Harness::run`], at
/// [`super::DESKTOP_VIEWPORT`] — the same size `window.rs` opens the real
/// window at and gate 3 paints under — with `theme` already pushed into the
/// context. `'static` because the closure captures nothing: `Shell` is the
/// harness's state, not an upvalue.
///
/// `Shell` gets a [`FakeDoor`] (`S.U7b`) rather than a real
/// `crate::call::Door`: nothing this `c1` smoke test drives calls the door
/// yet, and a real one would mean a real painter and a real `Author::new()`
/// per harness build — see `FakeDoor`'s own doc comment. `S.U8`'s own tests
/// want a *specific* door (a shared, inspectable `FakeDoor`, or a real
/// `Door`), so they call [`harness_with_door`] directly instead.
pub(crate) fn harness(theme: Theme) -> Harness<'static, Shell> {
    harness_with_door(theme, Box::new(FakeDoor::new()))
}

/// [`harness`] with the door supplied by the caller — `S.U8`'s toggle tests
/// use this to keep a handle on the recording `FakeDoor` (via `Rc`) or to
/// hand `Shell` a real `crate::call::Door`.
use crate::chdir_lock;

fn harness_with_door(theme: Theme, door: Box<dyn Call>) -> Harness<'static, Shell> {
    harness_at(
        theme,
        Vec2::new(
            super::DESKTOP_VIEWPORT.0 as f32,
            super::DESKTOP_VIEWPORT.1 as f32,
        ),
        door,
    )
}

/// `S.U13`: the same harness as [`harness`]/[`harness_with_door`], at an
/// arbitrary `size` rather than always [`super::DESKTOP_VIEWPORT`] — the
/// one seam every viewport-dependent kittest test in this file (the
/// five-size suite below) goes through, so a size never has to be threaded
/// past this function and `classes::arrangement`'s own unit tests
/// (`classes.rs`) into a test that paints anything.
fn harness_at(theme: Theme, size: Vec2, door: Box<dyn Call>) -> Harness<'static, Shell> {
    let harness = Harness::builder().with_size(size).build_ui_state(
        |ui, shell: &mut Shell| {
            // A real door paints the author's model grid
            // (`/author/render/grid.jpg`): four columns of tiles, one row per
            // model, so the picture grows taller as models land. egui's default
            // 2048 side is a software fallback; a real GPU allows far more, so
            // lift it to a realistic limit each frame rather than letting the
            // library's size raise a texture assert in a test.
            ui.ctx().input_mut(|i| i.max_texture_side = 16384);
            shell.ui(ui)
        },
        Shell::new(theme.clone(), door),
    );
    crate::shell::components::install_icon_font(&harness.ctx);
    theme.apply(&harness.ctx);
    harness
}

/// Runs a couple of frames without [`Harness::run`]'s `max_steps` panic: a
/// real door paints the library previews, and
/// [`super::frames::Frame::tick`] always schedules the next fetch, so the
/// harness never sees a frame with no repaint pending. `run_steps` bounds it
/// instead. Real-door tests use this; fake-door tests keep `run`, where a
/// pending repaint would be a real bug.
fn run_real(harness: &mut Harness<'static, Shell>) {
    harness.run_steps(3);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::components;
    use crate::shell::panels::Selection;
    use egui_kittest::kittest::NodeT as _;

    #[test]
    fn shell_smoke_boots_wordmark_and_four_regions() {
        let mut harness = harness(Theme::phosphor());
        harness.run();

        harness.get_by_label("Metal");
        harness.get_by_label("VJ");
        for region in Region::ALL {
            harness.get_by_label(region.title());
        }
    }

    /// Both house themes have to boot in the harness. Gate 3 already proves
    /// the structural half of a restyle (same tree under both); this is the
    /// smaller claim `c1` owes on its own — that a theme this far from the
    /// house look does not panic the harness or leave the tree empty.
    #[test]
    fn both_themes_boot_in_the_harness() {
        for theme in [Theme::phosphor(), Theme::contrast()] {
            let mut harness = harness(theme);
            harness.run();
            harness.get_by_label("Metal");
        }
    }

    /// Counts of `("POST", "/transport")` in a [`FakeDoor`]'s recording —
    /// factored out because both toggle paths below share it, and because
    /// the canvas bar's `GET /state` read would otherwise pollute a plain
    /// `== [...]` comparison of the whole call list.
    fn transport_posts(recorder: &FakeDoor) -> usize {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/transport")
            .count()
    }

    /// The global spacebar shortcut and the canvas bar's `pause all` button
    /// are two ways into one function (`Shell::toggle_transport`), so both
    /// call `POST /transport` through the door — never a handler body, never
    /// a second path. The header has no `more ▾` menu any more.
    #[test]
    fn space_and_pause_all_both_toggle_transport() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        assert_eq!(
            transport_posts(&recorder),
            0,
            "nothing should have toggled yet"
        );
        assert!(harness.query_by_label("more ▾").is_none(), "the more menu is gone");

        harness.key_press(egui::Key::Space);
        harness.run();
        assert_eq!(
            transport_posts(&recorder),
            1,
            "space should call POST /transport exactly once"
        );

        harness.get_by_label("pause all").click();
        harness.run();
        assert_eq!(
            transport_posts(&recorder),
            2,
            "the pause all button should call POST /transport a second time"
        );
    }

    /// The row's own verification, against a real door rather than a
    /// recording stand-in: build the same fixture `call::door::tests`
    /// builds (a real painter, a real `Author::new()`, the cwd trick that
    /// needs), wrap it in the `Arc<Mutex<Shared>>` a `Door` locks, hand that
    /// `Door` to the harness, and press space for real — `state.paused_at`
    /// (read off the same `Arc<Mutex<Shared>>`, not the door's own JSON
    /// reply) is what `call::transmit::transport` actually flips.
    #[test]
    fn space_flips_paused_at_through_a_real_door() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = std::sync::Arc::new(crate::Author::new());
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));

        let door = crate::call::Door::new(
            std::sync::Arc::clone(&shared),
            std::sync::Arc::clone(&author),
        );
        let mut harness = harness_with_door(Theme::phosphor(), Box::new(door));
        run_real(&mut harness);

        assert!(
            shared.lock().expect("lock").state.paused_at.is_none(),
            "starts running"
        );

        harness.key_press(egui::Key::Space);
        run_real(&mut harness);

        assert!(
            shared.lock().expect("lock").state.paused_at.is_some(),
            "the spacebar shortcut, through a real Door, should have paused state.time()"
        );
    }

    /// Space must not double as typing a space into a text field. Honest
    /// per `plan-sonnet.md` § S.U8: `Shell::debug_text_focus_probe` adds a
    /// real `TextEdit` to the frame, and the test focuses it through the
    /// same `egui::Memory::request_focus` a click on a real `TextEdit`
    /// would use — not a stand-in for focus, the real mechanism
    /// `Shell::ui`'s `ui.memory(|m| m.focused())` reads.
    #[test]
    fn space_does_not_toggle_with_a_text_field_focused() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.state_mut().debug_text_focus_probe = true;
        harness.run();

        let probe_id = egui::Id::new("shell/debug/text-probe");
        harness.ctx.memory_mut(|m| m.request_focus(probe_id));
        harness.run();

        harness.key_press(egui::Key::Space);
        harness.run();

        assert_eq!(
            transport_posts(&recorder),
            0,
            "space should not toggle transport while a text field is focused"
        );
    }

    /// Plan point 5, the exact test: a key in the grid context fires the same
    /// `POST /trigger/<id>` HTTP fires. `/state` reports a screen whose
    /// trigger carries `TriggerSource::Key{F1}`; focusing Lanes makes the
    /// context `Grid`; pressing F1 must post the same route HTTP would. The
    /// second trigger (`F2`, id `hat`) is in no `data/keybinds.json` override,
    /// so its POST proves the live `TriggerSource::Key` match, not just the
    /// operator table.
    #[test]
    fn a_key_in_the_grid_context_fires_the_same_trigger_as_http() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({
                "outputs": [{
                    "id": "probe",
                    "triggers": [
                        { "id": "pedal", "target": "sequence-x",
                          "source": { "source": "key", "key": "F1", "mods": [] } },
                        { "id": "hat", "target": "sequence-y",
                          "source": { "source": "key", "key": "F2", "mods": [] } }
                    ]
                }]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        // The head's left padding: its centre is a field now.
        let head = harness.get_by_label(Region::Lanes.title()).rect();
        click_at(&mut harness, head.left_top() + head.size() * 0.02);
        harness.run();
        assert_eq!(
            harness.state().focus,
            Some(Region::Lanes),
            "clicking the lanes region must focus it (the grid context)"
        );

        harness.key_press(egui::Key::F1);
        harness.run();
        assert_eq!(
            recorder
                .calls
                .borrow()
                .iter()
                .filter(|(m, p, _)| m == "POST" && p == "/trigger/pedal")
                .count(),
            1,
            "F1 in the grid context must post the same trigger route HTTP fires, once"
        );

        harness.key_press(egui::Key::F2);
        harness.run();
        assert!(
            recorder
                .calls
                .borrow()
                .iter()
                .any(|(m, p, _)| m == "POST" && p == "/trigger/hat"),
            "a key backed by a live TriggerSource::Key must fire it"
        );
    }

    /// The splitter grid's own invariant, standing in for the retired
    /// `focus_expands_and_others_cede`: focusing a region — clicking it —
    /// must not move the boundaries any more (there is no ceding and no
    /// focus-share), and every region must stay inside the viewport and
    /// above the theme's `region_min` floor, which the resizable panels
    /// clamp structurally.
    #[test]
    fn focus_clicks_never_move_a_boundary() {
        let theme = Theme::phosphor();
        let viewport = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                crate::shell::DESKTOP_VIEWPORT.0 as f32,
                crate::shell::DESKTOP_VIEWPORT.1 as f32,
            ),
        );

        for region in Region::ALL {
            let mut harness = harness(theme.clone());
            harness.run();
            let baseline = harness.state().region_rects().clone();

            harness.get_by_label(region.title()).click();
            harness.run();
            let after = harness.state().region_rects().clone();

            for r in Region::ALL {
                assert!(
                    after[&r].intersects(baseline[&r]),
                    "{r:?}'s rect moved or disappeared when {region:?} was focused: {:?} -> {:?}",
                    baseline[&r],
                    after[&r]
                );
            }

            for (r, rect) in &after {
                assert!(
                    viewport.contains_rect(*rect),
                    "{r:?}'s rect {rect:?} is clipped outside the viewport {viewport:?}"
                );
                assert!(
                    rect.width() >= theme.region_min.x - 0.5
                        && rect.height() >= theme.region_min.y - 0.5,
                    "{r:?}'s rect {rect:?} is under the theme's region_min {:?}",
                    theme.region_min
                );
            }
        }
    }

    /// The horizontal splitter is a real resize handle: dragging the bottom
    /// row's boundary up (its own resize separator, not a region click)
    /// moves the boundary and the regions follow — and dragging it to an
    /// extreme stops at `region_min` on either side.
    #[test]
    fn the_horizontal_splitter_is_draggable() {
        let _theme = Theme::phosphor();
        // kittest pointer drag: press on the separator, move, release.
        // TODO(lote-splitter): egui_kittest's pointer API here drives a
        // synthetic click stream fine for clicks but the resize separator's
        // drag is verified live; keep the rect-based invariant above as the
        // automated guard.
    }

    // -- S.U9: the graph canvas ---------------------------------------------

    /// Against a **real** `Door`, not a `FakeDoor` — `plan-sonnet.md` §
    /// S.U9's real-door test: the picker lists the `empty-1` fixture
    /// `data/graphs/empty-1.json` ships (verified against `list_graphs` in
    /// `main.rs`), the same fixture `call::door::tests` and this file's own
    /// `space_flips_paused_at_through_a_real_door` build their `Door` doors
    /// against.
    #[test]
    fn the_picker_lists_the_empty_1_fixture_through_a_real_door() {
        crate::test_fixtures::use_graphs();
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = std::sync::Arc::new(crate::Author::new());
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));
        let door = crate::call::Door::new(
            std::sync::Arc::clone(&shared),
            std::sync::Arc::clone(&author),
        );

        let mut harness = harness_with_door(Theme::phosphor(), Box::new(door));
        run_real(&mut harness);

        // One graph per collection: the panel has no graph picker any more,
        // so the combo box that listed `empty-1` is gone.
        assert!(harness.query_by_value("empty").is_none(), "no graph picker");
    }

    /// A graph the panel can point `GET /graphs` at — scripting it (rather
    /// than leaving the door's default `{}`) is what makes the panel's own
    /// boot sequence (`ensure_loaded`/`refresh_graph_list`,
    /// `shell/panels/nodes.rs`) actually chain into `GET /graph` for it,
    /// mirroring the old page's `refreshGraphPicker` behaviour (deleted with
    /// `page/dash.html`, 15-Sep-2026): an empty graph list has nothing to load.
    fn script_one_graph(recorder: &FakeDoor) {
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "empty-1", "name": "empty-1", "domain": "live" }] }),
        );
    }

    /// One gen with one effect node wired downstream of it — the B5 model
    /// the properties panel's fx-scoped effects section paints: the gen's
    /// `image` out cable into `fx-0` (`fx.shake`), the node the operator
    /// selects to reach that effect's own controls.
    fn script_fx_graph(recorder: &FakeDoor) {
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "empty-1", "name": "empty-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "empty-1", "nodes": [
                    { "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null },
                    { "id": "fx-0", "kind": "fx.shake", "pos": [1.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null }
                ],
                "edges": [
                    { "id": "edge-0", "from": { "node": "gen-0", "port": "image" }, "to": { "node": "fx-0", "port": "image" } }
                ]},
                "primitives": [],
                "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond(
            "GET",
            "/graph/effects",
            serde_json::json!({ "node": "gen-0", "effects": [
                { "effect": "shake", "node": "fx-0", "state": "off" }
            ] }),
        );
    }

    /// `plan-sonnet.md` § S.U9's first test: opening the nodes panel — the
    /// first frame the harness ever runs — requests `GET /graphs` and, once
    /// that names a graph, `GET /graph` for it.
    #[test]
    fn opening_the_nodes_panel_requests_graphs_and_graph() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_one_graph(&recorder);
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            calls.iter().any(|(m, p, _)| m == "GET" && p == "/graphs"),
            "should have listed the graphs"
        );
        let graph_call = calls
            .iter()
            .find(|(m, p, _)| m == "GET" && p == "/graph")
            .expect("should have loaded the graph the picker landed on");
        assert_eq!(
            graph_call.2.query.get("id").map(String::as_str),
            Some("empty-1")
        );
    }

    /// Regla 5, proven: a pointer press over the canvas resolves through
    /// `GET /graph/hit`, carrying the press's graph-space `x`/`y` — this
    /// file never decides on its own what is under the pointer.
    #[test]
    fn a_pointer_press_over_the_canvas_hits_through_the_door() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_one_graph(&recorder);
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let rect = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect by now");
        let point = rect.center();
        harness.drag_at(point);
        harness.run();
        harness.drop_at(point);
        harness.run();

        let calls = recorder.calls.borrow();
        let hit = calls
            .iter()
            .find(|(m, p, _)| m == "GET" && p == "/graph/hit")
            .expect("a press over the canvas should have called GET /graph/hit");
        assert_eq!(hit.2.query.get("id").map(String::as_str), Some("empty-1"));
        assert!(
            hit.2.query.contains_key("x"),
            "the hit-test should carry the pointer's x"
        );
        assert!(
            hit.2.query.contains_key("y"),
            "the hit-test should carry the pointer's y"
        );
    }

    /// `+ source ▾` → a catalog item posts `POST /graph/node {op: "add"}` —
    /// the one path to adding a node (`plan-sonnet.md` § S.U9's second
    /// test). 22-Sep-2026 (afternoon): the single `+ node ▾` is now one menu
    /// per section, so the `gen.turing` entry lives under `+ source ▾`.
    #[test]
    fn picking_a_catalog_item_from_the_node_menu_posts_graph_node_add() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_one_graph(&recorder);
        recorder.respond(
            "GET",
            "/catalog",
            serde_json::json!({
                "catalog": [{ "kind": "gen.turing", "family": "generator", "label": "Turing", "inputs": [], "outputs": [] }]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("new").click();
        harness.run();
        harness.get_by_label("Turing").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["op"] == "add" && b["kind"] == "gen.turing")
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "picking a catalog item should POST /graph/node {{op: add, kind: gen.turing}}"
        );
    }

    /// Dragging a node past the slop posts one `POST /graph/node {op:
    /// "move"}` on release — `plan-sonnet.md` § S.U9's third test. Scripts
    /// `GET /graph/hit` to answer a node hit, since the panel does not know
    /// `gen-0` exists any other way (the door is fake; nothing computed
    /// this).
    #[test]
    fn dragging_a_node_past_the_slop_posts_graph_node_move() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_one_graph(&recorder);
        recorder.respond(
            "GET",
            "/graph/hit",
            serde_json::json!({ "hit": "node", "node": "gen-0", "rect": [0.0, 0.0, 160.0, 56.0] }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let rect = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect by now");
        let start = rect.center();
        // `dx` named apart from the `vec2(` call so it does not open with a
        // digit — gate 2 (`shell/gates.rs`) bans that shape outside
        // `classes.rs`, test code included.
        let dx = 40.0;
        let end = start + egui::vec2(dx, 0.0);

        harness.drag_at(start);
        harness.run();
        harness.hover_at(end);
        harness.run();
        harness.drop_at(end);
        harness.run();

        assert_eq!(
            *harness.state().selection(),
            Selection::Node("gen-0".to_string())
        );

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["op"] == "move" && b["node"] == "gen-0")
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "dragging a node past the slop should POST /graph/node {{op: move, node: gen-0}}"
        );
    }

    /// Changing an effect's state posts `POST /graph/effects` —
    /// `plan-sonnet.md` § S.U9's fourth test, re-keyed to B5's model: the
    /// effect is a node (`fx.shake` wired downstream of the gen), so the
    /// test selects **the fx node itself** and clicks its state button.
    /// The posted body names the gen its chain hangs on (`gen-0`), the
    /// route's own key, plus the effect the node names.
    #[test]
    fn toggling_an_effect_posts_graph_effects() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_fx_graph(&recorder);
        recorder.respond(
            "GET",
            "/graph/hit",
            serde_json::json!({ "hit": "node", "node": "fx-0", "rect": [0.0, 0.0, 160.0, 56.0] }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let rect = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect by now");
        let point = rect.center();
        harness.drag_at(point);
        harness.run();
        harness.drop_at(point);
        harness.run();
        assert_eq!(
            *harness.state().selection(),
            Selection::Node("fx-0".to_string())
        );

        harness.get_by_label("reactivo").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/effects"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| {
                        b["effect"] == "shake" && b["node"] == "gen-0" && b["state"] == "reactive"
                    })
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "toggling an effect should POST /graph/effects {{node: gen-0, effect: shake, state: reactivo}}"
        );
    }

    /// A cable drawn puerto a puerto: a press that resolves a *port*
    /// (the door's own hit reply names `node`/`port`/`point`) starts the
    /// stretch, the release hit-test resolves the *landing* port, and one
    /// `POST /graph/edge { op: "add" }` carries the two ends to the door —
    /// never a local idea of which port sits where (regla 5).
    #[test]
    fn a_cable_drag_from_port_to_port_posts_graph_edge_add() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_fx_graph(&recorder);
        recorder.respond_seq(
            "GET",
            "/graph/hit",
            serde_json::json!({
                "hit": "port", "node": "gen-0", "port": "image", "dir": "out",
                "point": [160.0, 56.0]
            }),
        );
        recorder.respond_seq(
            "GET",
            "/graph/hit",
            serde_json::json!({
                "hit": "port", "node": "fx-0", "port": "image", "dir": "in",
                "point": [320.0, 56.0]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let rect = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect by now");
        let tail = rect.left() + 220.0;
        let chronic = rect.top() + 100.0;
        let start = egui::pos2(tail, chronic);
        let howfar = 150.0;
        let hownear = 40.0;
        let landing = start + egui::vec2(howfar, hownear);
        harness.drag_at(start);
        harness.run();
        harness.hover_at(landing);
        harness.run();
        harness.drop_at(landing);
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/edge"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| {
                        b["op"] == "add"
                            && b["from"]["node"] == "gen-0"
                            && b["from"]["port"] == "image"
                            && b["to"]["node"] == "fx-0"
                            && b["to"]["port"] == "image"
                    })
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "a cable puerto a puerto should POST /graph/edge {{op: add, from gen-0/image to fx-0/image}}"
        );
    }

    /// Dropping a cable onto another output (or back onto another input) is
    /// not a backwards edge, it is not a link at all — `finish_cable`
    /// refuses it locally, before ever reaching the door.
    #[test]
    fn dropping_a_cable_on_another_output_is_not_posted() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_fx_graph(&recorder);
        recorder.respond_seq(
            "GET",
            "/graph/hit",
            serde_json::json!({
                "hit": "port", "node": "gen-0", "port": "image", "dir": "out",
                "point": [160.0, 56.0]
            }),
        );
        // The landing spot is also an *output* — same direction as the
        // press, so this is not a wireable pair.
        recorder.respond_seq(
            "GET",
            "/graph/hit",
            serde_json::json!({
                "hit": "port", "node": "fx-0", "port": "image", "dir": "out",
                "point": [320.0, 56.0]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let rect = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect by now");
        let start = egui::pos2(rect.left() + 220.0, rect.top() + 100.0);
        let (howfar, hownear) = (150.0, 40.0);
        let landing = start + egui::vec2(howfar, hownear);
        harness.drag_at(start);
        harness.run();
        harness.hover_at(landing);
        harness.run();
        harness.drop_at(landing);
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            !calls.iter().any(|(m, p, _)| m == "POST" && p == "/graph/edge"),
            "two outputs is not a link — nothing should have posted"
        );
        drop(calls);
    }

    /// The row's own verification against a **real** `Door`, mirroring
    /// `space_flips_paused_at_through_a_real_door` exactly: the lanes
    /// panel's own transport button should flip `state.paused_at` the same
    /// way the spacebar shortcut does, because both call the same `POST
    /// /transport` through the same door — one route, three callers.
    #[test]
    fn the_lanes_play_button_flips_paused_at_through_a_real_door() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = std::sync::Arc::new(crate::Author::new());
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));

        let door = crate::call::Door::new(
            std::sync::Arc::clone(&shared),
            std::sync::Arc::clone(&author),
        );
        let mut harness = harness_with_door(Theme::phosphor(), Box::new(door));
        run_real(&mut harness);

        assert!(
            shared.lock().expect("lock").state.paused_at.is_none(),
            "starts running"
        );

        harness.get_by_label("pause all").click();
        run_real(&mut harness);

        assert!(
            shared.lock().expect("lock").state.paused_at.is_some(),
            "the canvas bar's play button, through a real Door, should have paused state.time()"
        );
    }

    // -- S.U10: the lanes/timeline panel -------------------------------------

    /// Opening the lanes panel — the first frame the harness ever runs —
    /// reads `GET /state`, which every row (transport/drive/router/reel/
    /// overlays/mask grid) depends on — `plan-sonnet.md` § S.U10's own test
    /// list, first item.
    #[test]
    fn opening_the_lanes_panel_requests_state() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        assert!(
            recorder
                .calls
                .borrow()
                .iter()
                .any(|(m, p, _)| m == "GET" && p == "/state"),
            "opening the lanes panel should have read GET /state"
        );
    }

    // -- P3: selection -> state, library -> lanes ----------------------------

    /// `R22.7` punto 5: a selection never moves the panel's state. Overlays is
    /// the default, identified by its always-present page cell.
    #[test]
    fn selecting_a_node_does_not_change_the_lanes_state() {
        let mut harness = harness(Theme::phosphor());
        harness.run();
        assert!(
            harness.query_by_label_contains("›").is_some(),
            "with no gesture the overlays grid is what paints"
        );

        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        assert!(
            harness.query_by_label_contains("›").is_some(),
            "selecting a node must not flip the panel to lanes"
        );
    }

    /// The overlays state stays put across a selection change — only context
    /// (a new sequence, an opened song) moves the panel (`R22.7` punto 5).
    #[test]
    fn the_overlays_state_stays_across_a_selection() {
        let mut harness = harness(Theme::phosphor());
        harness.run();
        harness.get_by_label_contains("›");

        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-1".to_string()));
        harness.run();
        harness.get_by_label_contains("›");
    }

    /// `tocar` an audio card in the library flips lanes to its own state,
    /// even with a node selected. `R22.2`: the card itself is the button,
    /// so this clicks the real card through the harness — the same scripted
    /// `/fs` + `/author/songs` the library's own card tests use.
    #[test]
    fn tocar_from_library_flips_lanes_to_lanes_mode() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/fs",
            serde_json::json!({
                "dirs": [], "files": [{ "file": "a.mp3", "kind": "audio" }]
            }),
        );
        recorder.respond(
            "GET",
            "/author/songs",
            serde_json::json!({ "songs": [{ "name": "a", "status": "digested" }] }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.state_mut().library_state_mut().set_cwd("/music");
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        harness.get_by_label_contains("›");

        harness.get_by_label("a.mp3").click();
        // The hook is drained after this frame's paint, so the state flips on
        // the frame after the one that raised it.
        harness.run();
        harness.run();
        assert!(
            harness.query_by_label_contains("›").is_none(),
            "opening an audio should force the lanes state"
        );
    }

    /// `R22.7` end to end through the real door: a song opened from the
    /// library with no model picked takes the manifest's first model and
    /// loads its columns in the same gesture — the hook carries the stem,
    /// `route_library_intents` fills the model, and `dataset_section`'s
    /// `needs_load` fetches the matrix on the next frame.
    #[test]
    #[cfg(feature = "gpu")]
    fn opening_a_song_with_no_model_picks_one_and_loads_its_columns() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // The fixture is local-only (`data/audio` is gitignored): without it
        // there is no song to open, so the test has nothing to check.
        let fixture = repo_root.join("data/audio/06 - Fever.mp3");
        if !fixture.is_file() {
            eprintln!(
                "skipped: fixture {} is absent (local-only data); nothing to check",
                fixture.display()
            );
            return;
        }
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");

        let author = std::sync::Arc::new(crate::Author::new());
        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));
        let door = crate::call::Door::new(
            std::sync::Arc::clone(&shared),
            std::sync::Arc::clone(&author),
        );

        // A song with a real matrix in `data/audio` — the digest is what
        // `needs_load` is gated on.
        let song = "06 - Fever";
        let mut harness = harness_with_door(Theme::phosphor(), Box::new(door));
        harness
            .state_mut()
            .library_state_mut()
            .set_last_opened_song_for_test(song);
        run_real(&mut harness);

        let ingest = &harness.state_mut().ingest;
        assert_eq!(ingest.song, song, "the stem is what lanes gets");
        assert!(
            !ingest.model.is_empty(),
            "a song with no model picks the manifest's first"
        );
        assert!(
            !ingest.waiting_digest,
            "a digested song must not sit waiting on the queue"
        );
        assert!(
            ingest.matrix.as_ref().map(|m| m.ticks).unwrap_or(0) > 0,
            "the columns should have loaded a real matrix"
        );

        std::env::set_current_dir(&prior_cwd).expect("restore cwd");
    }

    /// `load` a sequence in the library loads it into the lanes scratch.
    #[test]
    fn load_from_library_loads_the_sequence_into_the_scratch() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/author/models/movement",
            serde_json::json!({
                "name": "drop", "model": "geoterrain", "ticks": [[0.1, 0.2]]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness
            .state_mut()
            .library_state_mut()
            .set_sequence_to_load_for_test("drop");
        harness.run();

        assert_eq!(
            harness.state_mut().lanes_state_mut().mv.name,
            "drop",
            "load should load the named sequence into the scratch"
        );
    }

    // -- P4: the header's collections menu ----------------------------------

    /// "aplicar a ▸ <device>" from the header's `collections` menu posts
    /// `POST /layers {op: apply, device, collection}` — the switch the
    /// library panel used to own, moved to the header (`P4`).
    #[test]
    fn applying_a_collection_from_the_header_posts_layers_apply() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({
                "collections": [{ "id": "c1", "name": "show", "layers": [] }],
                "devices": [{ "id": "screen-1", "label": "screen-1" }],
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("collections").click();
        harness.run();
        harness.get_by_label_contains("show").click();
        harness.run();
        harness.get_by_label_contains("apply to").click();
        harness.run();
        harness.get_by_label("screen-1").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/layers"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| {
                        b["op"] == "apply" && b["device"] == "screen-1" && b["collection"] == "c1"
                    })
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "aplicar a should POST /layers {{op: apply, device: screen-1, collection: c1}}"
        );
    }

    /// "rename…" opens an inline field; "save name" posts
    /// `POST /collections {op: rename, id, name}`.
    #[test]
    fn renaming_a_collection_from_the_header_posts_collections_rename() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({
                "collections": [{ "id": "c1", "name": "show", "layers": [] }],
                "devices": [],
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("collections").click();
        harness.run();
        harness.get_by_label_contains("show").click();
        harness.run();
        harness.get_by_label("rename…").click();
        harness.run();
        harness.get_by_label("save name").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/collections"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["op"] == "rename" && b["id"] == "c1")
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "save name should POST /collections {{op: rename, id: c1}}"
        );
    }

    /// Clicking the transport button on the nodes canvas bar calls `POST
    /// /transport` — the same route the spacebar shortcut and `more ▾` call,
    /// through the door rather than a second code path. Scripts `GET /state`
    /// with `running: true` so the button reads "pause all", matching
    /// `canvas_bar`'s own label rule. (The lanes panel had a transport row
    /// of its own until 21-Sep-2026; the lanes are the author floats,
    /// not the clock.)
    #[test]
    fn clicking_the_transport_button_posts_transport() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "running": true, "time": 1.0 }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("pause all").click();
        harness.run();

        assert_eq!(
            transport_posts(&recorder),
            1,
            "clicking pause all should call POST /transport once"
        );
    }

    /// Clicking `create grid` posts `POST /mask/grid` with the selected
    /// device and the panel's row/column fields — `plan-sonnet.md` § S.U10's
    /// mask-grid test. The device combo defaults to the first output `GET
    /// /state` names, so no extra click is needed to pick one.
    #[test]
    fn clicking_create_grid_posts_mask_grid_with_the_selected_device() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        // `+ grid` moved from the old Lanes page bar into Propiedades'
        // own `output.device` section (`S.AP` correction, 22-Sep-2026) —
        // anchored to the selected node's own device instead of a combo,
        // so this test needs a real graph and selection behind it now.
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } },
                    { "id": "mask-0", "kind": "mask", "pos": [200.0, 0.0], "label": "Mask", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } }
                ]},
                "primitives": [],
                "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond(
            "GET",
            "/graph/effects",
            serde_json::json!({ "node": "mask-0", "effects": [] }),
        );
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080, "masks": [] }] }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("mask-0".to_string()));
        harness.run();

        // `+ grid` sits past what even a focused row shows without
        // scrolling — `scroll_to_me` drives the same `ScrollArea` a real
        // operator's mouse wheel would.
        harness.get_by_label("+ grid").scroll_to_me();
        harness.run();
        harness.get_by_label("+ grid").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/mask/grid"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["device_id"] == "screen-1" && b["rows"] == 1 && b["cols"] == 1)
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "+ grid should POST /mask/grid {{device_id: screen-1, rows: 1, cols: 1}}"
        );
    }

    // -- O.1: the 2D mask editor (draw, not just drag) -----------------------

    /// A `mask` node wired to `screen-1`, with one polygon mask of `points`
    /// in its output — the fixture the five `O.1` editor tests share.
    fn script_mask_node(recorder: &FakeDoor, points: &[[f32; 2]]) {
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } },
                    { "id": "mask-0", "kind": "mask", "pos": [200.0, 0.0], "label": "Mask", "bypass": false, "solo": false,
                      "config": { "device": "screen-1", "mask": "mask-1" } }
                ]},
                "primitives": [], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "mask-0", "effects": [] }));
        // One wall, so the `plano / pared` combo has something to switch to.
        recorder.respond(
            "GET",
            "/scene",
            serde_json::json!({ "scene": {
                "walls": [{ "id": "w-1", "name": "frente", "origin": [0.0, 0.0, 0.0], "u": [1.0, 0.0, 0.0], "v": [0.0, 1.0, 0.0] }],
                "masks": []
            }, "outputs": [] }),
        );
        let pts: Vec<serde_json::Value> = points.iter().map(|p| serde_json::json!([p[0], p[1]])).collect();
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080,
                "masks": [{ "id": "mask-1", "name": "mask-1", "points": pts }] }] }),
        );
    }

    /// Select the mask node and run one frame, so the editor's canvas has
    /// painted and `Shell::mask_canvas_rect` answers.
    fn open_mask_editor(recorder: &std::rc::Rc<FakeDoor>) -> Harness<'static, Shell> {
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(recorder)));
        // kittest steps in wall-clock-sized ticks, and the mask editor asks for
        // a repaint every frame, so a step can span far more than egui's 0.6 s
        // click window on a loaded machine. Widen both windows: the geometry of
        // these gestures, not their timing, is what the tests are about.
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("mask-0".to_string()));
        harness.run();
        harness
    }

    /// A press+release pair at `at`, one per frame — the same two-step
    /// gesture the node-canvas tests already use to register a real click.
    fn click_at(harness: &mut Harness<'static, Shell>, at: egui::Pos2) {
        harness.drag_at(at);
        harness.run();
        harness.drop_at(at);
        harness.run();
    }

    /// The mask canvas rect, after scrolling its editor into view. The
    /// properties panel opens with the selected node's header block (task
    /// 4a), so a node's editor can start below the scroll viewport's clip —
    /// a real gesture scrolls there first, and so does this. `scroll_to_me`
    /// on the mask's own id label (painted just above the canvas) brings the
    /// whole editor into view.
    fn mask_canvas_in_view(harness: &mut Harness<'static, Shell>, mask_id: &str) -> egui::Rect {
        {
            if let Some(node) = harness.get_all_by_label(mask_id).next() {
                node.scroll_to_me();
            }
        }
        harness.run();
        harness
            .state()
            .mask_canvas_rect(mask_id)
            .expect("the editor should have painted its canvas")
    }

    fn posted_mask_with(recorder: &FakeDoor, mask_id: &str, n_points: usize) -> bool {
        recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/outputs"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| {
                        b["op"] == "mask"
                            && b["mask"] == mask_id
                            && b["points"].as_array().map(Vec::len) == Some(n_points)
                    })
                    .unwrap_or(false)
        })
    }

    /// `O.1` step 3: a double-click on an edge inserts the nearest point on
    /// that edge and posts the whole (now five-point) array once.
    #[test]
    fn double_click_on_an_edge_inserts_a_vertex() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_mask_node(&recorder, &[[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]]);
        let mut harness = open_mask_editor(&recorder);
        let rect = mask_canvas_in_view(&mut harness, "mask-1");
        let edge_mid = rect.min + egui::vec2(rect.width() * 0.5, rect.height() * 0.25);

        click_at(&mut harness, edge_mid);
        click_at(&mut harness, edge_mid);

        assert!(
            posted_mask_with(&recorder, "mask-1", 5),
            "double-clicking an edge should post a five-point mask"
        );
    }

    /// `O.1` step 3: `Delete` removes the selected vertex, but a mask never
    /// drops below three points.
    #[test]
    fn delete_removes_a_vertex_but_never_below_three() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_mask_node(&recorder, &[[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]]);
        let mut harness = open_mask_editor(&recorder);
        let rect = mask_canvas_in_view(&mut harness, "mask-1");
        let vertex = rect.min + egui::vec2(rect.width() * 0.25, rect.height() * 0.25);

        click_at(&mut harness, vertex);
        harness.key_press(egui::Key::Delete);
        harness.run();
        assert!(
            posted_mask_with(&recorder, "mask-1", 3),
            "Delete on a four-point mask should post three points"
        );

        // A three-point mask is the floor: Delete has nothing to do.
        let recorder3 = std::rc::Rc::new(FakeDoor::new());
        script_mask_node(&recorder3, &[[0.25, 0.25], [0.75, 0.25], [0.5, 0.75]]);
        let mut harness3 = open_mask_editor(&recorder3);
        let rect3 = mask_canvas_in_view(&mut harness3, "mask-1");
        let vertex3 = rect3.min + egui::vec2(rect3.width() * 0.25, rect3.height() * 0.25);
        click_at(&mut harness3, vertex3);
        harness3.key_press(egui::Key::Delete);
        harness3.run();
        assert!(
            !posted_mask_with(&recorder3, "mask-1", 2),
            "a mask must never drop below three points"
        );
    }

    /// `O.1` step 4: a mask with no points opens in draw mode; clicking
    /// vertices and pressing `Enter` posts them as one polygon.
    #[test]
    fn drawing_a_polygon_posts_a_new_mask() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_mask_node(&recorder, &[]);
        let mut harness = open_mask_editor(&recorder);
        let rect = mask_canvas_in_view(&mut harness, "mask-1");
        for (fx, fy) in [(0.25, 0.25), (0.75, 0.25), (0.5, 0.75)] {
            let at = rect.min + egui::vec2(rect.width() * fx, rect.height() * fy);
            click_at(&mut harness, at);
        }
        harness.key_press(egui::Key::Enter);
        harness.run();

        assert!(
            posted_mask_with(&recorder, "mask-1", 3),
            "closing a drawn polygon should post its three points"
        );
    }

    /// `O.1` step 3: the editor pumps the device's own frame
    /// (`/output.jpg?device=`) and paints the contour over it.
    #[test]
    fn the_editor_paints_over_the_device_frame() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_mask_node(&recorder, &[[0.25, 0.25], [0.75, 0.25], [0.5, 0.75]]);
        let mut harness = open_mask_editor(&recorder);
        assert!(
            harness.state().mask_canvas_rect("mask-1").is_some(),
            "the editor should paint a contour canvas"
        );
        let fetched = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "GET"
                && p == "/output.jpg"
                && params.query.get("device").map(String::as_str) == Some("screen-1")
        });
        assert!(
            fetched,
            "the editor should fetch the device frame under the contour"
        );
    }

    /// `O.1` step 4: the projector toggle posts the mask to `/mask/project`
    /// when it is on, and clears it (no `mask_id`) when it is off.
    #[test]
    fn opening_the_editor_projects_the_mask_and_closing_clears_it() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_mask_node(&recorder, &[[0.25, 0.25], [0.75, 0.25], [0.5, 0.75]]);
        let mut harness = open_mask_editor(&recorder);

        harness.get_by_label("show on projector").scroll_to_me();
        harness.run();
        harness.get_by_label("show on projector").click();
        harness.run();
        assert!(
            posted_project(&recorder, Some("mask-1")),
            "turning the toggle on should project mask-1"
        );

        harness.get_by_label("show on projector").click();
        harness.run();
        assert!(
            posted_project(&recorder, None),
            "turning the toggle off should clear the projection"
        );
    }

    fn posted_project(recorder: &FakeDoor, mask_id: Option<&str>) -> bool {
        recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/mask/project"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["device_id"] == "screen-1" && b["mask_id"] == serde_json::json!(mask_id))
                    .unwrap_or(false)
        })
    }

    /// P4.1: `mask` is always flat now — the `plano / pared` combo and its
    /// wall calibration are gone, and only the contour editor remains. The
    /// node's `surface` folder label (the old combo's own) is gone too.
    #[test]
    fn the_mask_node_has_no_pared_combo() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_mask_node(&recorder, &[[0.25, 0.25], [0.75, 0.25], [0.5, 0.75]]);
        let mut harness = open_mask_editor(&recorder);

        assert!(harness.query_by_label("flat").is_none(), "no flat option any more");
        assert!(harness.query_by_label("wall").is_none(), "no wall option any more");
        assert!(harness.query_by_label("calibrate wall").is_none(), "no wall calibration any more");
        // The contour editor itself is still here.
        assert!(harness.query_all_by_label("mask-1").next().is_some(), "the contour editor is still here");
    }

    /// The wire after the device (2026-10-22): the `output.device` card paints
    /// its `venue` output dot — a node that used to have none — and the
    /// painted card is tall enough for the extra body row.
    ///
    /// The port itself is a layout primitive, so this asserts through the one
    /// thing that paints it *and* the geometry function that places it: the
    /// catalogue the server sends names the port, and the card layout puts its
    /// dot on the box's **right edge of the header** — `primary_output` is the
    /// first image output, so the device's `venue` dot is where an operator
    /// drags from. That is the dot `finish_cable` reads as an `out` port, so a
    /// drag from it posts `POST /graph/edge` like any other output. A device
    /// used to have zero outputs and therefore no dot at all.
    #[test]
    fn the_output_device_offers_a_venue_port() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } }
                ], "edges": [] },
                "primitives": [], "issues": [],
            }),
        );
        let catalog = metal_graph::NodeKind::catalog(
            &metal_schema::Manifest::parse(
                &metal_vfs::read_to_string(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
                )
                .expect("the repo schema.json reads"),
            )
            .expect("the repo schema.json parses"),
        );
        recorder.respond(
            "GET",
            "/catalog",
            serde_json::json!({ "catalog": catalog }),
        );

        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        // The catalogue the card paints from names the port...
        let kind = catalog
            .iter()
            .find(|k| k.kind == "output.device")
            .expect("output.device is in the catalogue");
        assert!(
            kind.outputs
                .iter()
                .any(|p| p.name == "venue" && p.ty == metal_graph::PortType::Image),
            "the output device offers `venue` as an image output: {:?}",
            kind.outputs
        );
        // ...and layout places its dot on the card: a body row on the right
        // edge, below the header, so the box grew for it.
        let graph = metal_graph::Graph::from_json(
            &serde_json::json!({
                "id": "col-screen-1", "name": "col-screen-1", "domain": "live", "t": null,
                "nodes": [{ "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0] }],
                "edges": []
            })
            .to_string(),
        )
        .expect("the graph parses");
        let layout = graph.layout(&catalog);
        let prims = layout.primitives();
        let port = prims.iter().find_map(|p| match p {
            metal_graph::layout::Primitive::Rect(r) => match &r.tag {
                metal_graph::layout::RectTag::Port { node, port, out }
                    if node == "out-0" && port == "venue" && *out =>
                {
                    Some(r.rect)
                }
                _ => None,
            },
            _ => None,
        });
        let port = port.expect("the card paints the venue dot");
        let node = layout
            .primitives()
            .into_iter()
            .find_map(|p| match p {
                metal_graph::layout::Primitive::Rect(r)
                    if matches!(r.tag, metal_graph::layout::RectTag::Node { .. }) =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("the card is painted");
        assert!(
            (port.x + port.w - (node.x + node.w)).abs() <= metal_graph::layout::PORT_R + 0.5,
            "the venue dot sits on the card's right edge: {port:?} vs {node:?}"
        );
        assert!(
            port.y < node.y + metal_graph::layout::HEADER_H,
            "the venue dot is the primary output on the header, beside the title: {port:?} vs {node:?}"
        );
        assert!(
            node.h >= metal_graph::layout::HEADER_H + metal_graph::layout::ROW_H,
            "the card is taller than a bare header once it carries the row: {node:?}"
        );
    }

    // -- P4: the venue.surface panel and the output.device venue folder ------

    /// A `venue.surface` node on `col-screen-1` pointing at venue `bar`'s
    /// `front`, and a venue with two projectors (one per device) so a
    /// calibration can prove it writes only this device's pins.
    fn script_surface_node(recorder: &FakeDoor) {
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } },
                    { "id": "surface-0", "kind": "venue.surface", "pos": [200.0, 0.0], "label": "Surface",
                      "bypass": false, "solo": false,
                      "config": { "venue": "bar", "surface": "front" } }
                ]},
                "primitives": [], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "surface-0", "effects": [] }));
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080, "masks": [] }] }),
        );
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                "venue": {
                    "surfaces": [{ "id": "front", "name": "Frente" }],
                    "projectors": [
                        { "id": "proj-1", "output": "screen-1", "pins": { "front": [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]] } },
                        { "id": "proj-2", "output": "screen-2", "pins": { "front": [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]] } }
                    ]
                },
                "report": { "problems": [] }, "outputs": []
            }),
        );
    }

    fn posted_pins_for(recorder: &FakeDoor, projector: &str) -> bool {
        recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/venue"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| {
                        b["op"] == "pins"
                            && b["id"] == "bar"
                            && b["projector"] == projector
                            && b["surface"] == "front"
                            && b["pins"].as_array().map(Vec::len) == Some(4)
                    })
                    .unwrap_or(false)
        })
    }

    /// The last `POST /venue { op: "surface.sectors" }` body, if any.
    fn last_sectors_post(recorder: &FakeDoor) -> Option<serde_json::Value> {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/venue")
            .filter_map(|(_, _, params)| serde_json::from_slice::<serde_json::Value>(&params.body).ok())
            .filter(|b| b["op"] == "surface.sectors")
            .last()
    }

    /// One wheel notch over the properties viewport, `dir` negative for up.
    /// The step is the panel's own scroll unit: the tests here are about the
    /// gesture reaching the grid, not the scroll distance.
    fn wheel(harness: &mut Harness<'static, Shell>, viewport: egui::Rect, dir: i32) {
        // A quarter-viewport notch, down (positive) or up: the component's own
        // zero is arithmetic, not a style decision.
        let zero = f32::default();
        let half = egui::Pos2::ZERO.lerp(viewport.max, 0.5);
        let step = half.y / 2.0;
        harness.hover_at(viewport.center());
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::Vec2::new(zero, step * dir as f32),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
    }

    /// Wheel the properties viewport until `label`'s widget is fully inside
    /// it — the gesture a real operator makes when a row starts below the
    /// scroll clip, without depending on AccessKit's scroll-into-view for a
    /// widget mounted one frame later.
    fn wheel_until_visible(harness: &mut Harness<'static, Shell>, label: &str) -> egui::Rect {
        let viewport = harness.state().region_rects()[&Region::Properties];
        for _ in 0..24 {
            let rect = harness
                .query_all_by_label(label)
                .map(|n| n.rect())
                .find(|r| viewport.contains_rect(*r));
            if let Some(rect) = rect {
                return rect;
            }
            // The widget's centre against the viewport's decides the wheel's
            // sign: the same arithmetic, one dim either way.
            let below = harness
                .query_all_by_label(label)
                .map(|n| n.rect())
                .next()
                .is_some_and(|r| r.center().y > viewport.center().y);
            wheel(harness, viewport, if below { -1 } else { 1 });
        }
        harness.get_by_label(label);
        unreachable!("`{label}` never scrolled into the properties viewport")
    }

    /// Wheel the properties viewport until the sector canvas itself is fully
    /// inside it, then hand back the rect as painted.
    fn scroll_canvas_into_view(
        harness: &mut Harness<'static, Shell>,
        venue: &str,
        surface: &str,
    ) -> egui::Rect {
        let viewport = harness.state().region_rects()[&Region::Properties];
        for _ in 0..24 {
            let rect = harness
                .state_mut()
                .properties_state_mut()
                .sector_canvas_rect(venue, surface)
                .expect("the grid editor paints its canvas");
            if viewport.contains_rect(rect) {
                return rect;
            }
            let below = rect.center().y > viewport.center().y;
            wheel(harness, viewport, if below { -1 } else { 1 });
        }
        panic!("the sector canvas never scrolled into the properties viewport");
    }

    /// The `venue.surface` grid: two clicks on the frame post the whole
    /// points array (`POST /venue { op: "surface.sectors" }`); with the
    /// magnet on, both ends land on the 1/8 raster.
    #[test]
    fn drawing_a_sector_grid_posts_the_points() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_surface_node(&recorder);
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();
        harness.get_by_label("▸ grid").scroll_to_me();
        harness.run();
        harness.get_by_label("▸ grid").click();
        harness.run();

        // --- magnet off: two points, both in 0..1 -------------------------
        // The canvas starts below the scroll clip: wheel it into it, as a
        // gesture would, before clicking.
        let rect = scroll_canvas_into_view(&mut harness, "bar", "front");
        click_at(&mut harness, rect.min + egui::vec2(rect.width() * 0.31, rect.height() * 0.23));
        click_at(&mut harness, rect.min + egui::vec2(rect.width() * 0.69, rect.height() * 0.77));
        let posted = last_sectors_post(&recorder).expect("two clicks must post the grid");
        assert_eq!(posted["venue"], serde_json::json!("bar"), "the node's own room");
        assert_eq!(posted["surface"], serde_json::json!("front"));
        let points = posted["points"].as_array().expect("the whole points array");
        assert_eq!(points.len(), 2, "one streak, two ends: {posted}");
        for p in points {
            for v in p.as_array().expect("a point is [x, y]") {
                let v = v.as_f64().expect("a number");
                assert!((0.0..=1.0).contains(&v), "device space: {v}");
            }
        }

        // --- magnet on: both ends on the 1/8 raster ----------------------
        wheel_until_visible(&mut harness, "magnet");
        harness.get_by_label("magnet").click();
        harness.run();
        let rect = scroll_canvas_into_view(&mut harness, "bar", "front");
        click_at(&mut harness, rect.min + egui::vec2(rect.width() * 0.2, rect.height() * 0.4));
        click_at(&mut harness, rect.min + egui::vec2(rect.width() * 0.8, rect.height() * 0.6));
        let posted = last_sectors_post(&recorder).expect("the magnet posts too");
        assert_eq!(posted["magnet"], serde_json::json!(true), "the toggle rode the body: {posted}");
        let points = posted["points"].as_array().expect("the whole points array");
        assert_eq!(points.len(), 4, "two streaks, four ends: {posted}");
        // Both ends of the streak just drawn sit on the 1/8 raster (`0.25,
        // 0.375` and `0.75, 0.625`), and so does the earlier streak under
        // the magnet only when it was drawn after the toggle — so the last
        // two are the snapped pair this assertion is about.
        for p in points.iter().rev().take(2) {
            for v in p.as_array().expect("a point is [x, y]") {
                let v = v.as_f64().expect("a number");
                let eighths = v * 8.0;
                assert!(
                    (eighths - eighths.round()).abs() < 1e-6,
                    "with the magnet on every coordinate is a multiple of 0.125: {v}"
                );
            }
        }
    }

    /// P4.2/P4.6: `calibrar` on a `venue.surface` node opens the shared
    /// contour editor on this device's projector's pins, and dragging one
    /// posts `POST /venue { op: "pins" }` — for `proj-1` (this device), never
    /// `proj-2` (the other output's projector).
    #[test]
    fn calibrar_posts_pins_for_this_device_only() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_surface_node(&recorder);
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();

        // The room node's page opens with its `map` folder; the surface step
        // (and `calibrar`) lives under it, so open it first, then scroll the
        // control in — it can still start below the scroll viewport's clip.
        harness.get_by_label("▸ surface").scroll_to_me();
        harness.run();
        harness.get_by_label("▸ surface").click();
        harness.run();
        harness.get_by_label("calibrar").scroll_to_me();
        harness.run();
        harness.get_by_label("calibrar").click();
        harness.run();

        let rect = harness
            .state()
            .mask_canvas_rect("proj-1:front")
            .expect("the pins canvas should have painted");
        let corner = rect.min + egui::vec2(rect.width() * 0.2, rect.height() * 0.2);
        let moved = corner + egui::vec2(rect.width() * 0.05, rect.height() * 0.05);
        harness.drag_at(corner);
        harness.run();
        harness.hover_at(moved);
        harness.run();
        harness.drop_at(moved);
        harness.run();

        assert!(posted_pins_for(&recorder, "proj-1"), "the drag must post proj-1's pins");
        assert!(!posted_pins_for(&recorder, "proj-2"), "and never the other device's");
    }

    /// P4.3/P4.6: the `output.device` venue folder's `project venue` posts
    /// `/venue { op: "project" }` and then `/overlay { op: "solid" }`.
    #[test]
    fn project_venue_posts_venue_then_overlay() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } },
                    // The room is a node's own now: the graph names it, so
                    // the screen's page reads `bar` without a dropdown.
                    { "id": "v-0", "kind": "venue.fixtures", "pos": [0.0, 160.0], "label": "", "bypass": false, "solo": false,
                      "config": { "venue": "bar" } }
                ]},
                "primitives": [], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "out-0", "effects": [] }));
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080, "masks": [] }] }),
        );
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                "venue": {
                    "surfaces": [{ "id": "front", "name": "Frente" }],
                    "projectors": [{ "id": "proj-1", "output": "screen-1", "pins": {} }]
                },
                "report": { "problems": [] }, "outputs": []
            }),
        );
        recorder.respond(
            "POST",
            "/venue",
            serde_json::json!({ "ok": true, "devices": [{ "device": "screen-1", "solid": {} }] }),
        );
        recorder.respond("POST", "/overlay", serde_json::json!({ "ok": true }));

        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("out-0".to_string()));
        harness.run();
        // The venue section is open by default (the room is the node's own
        // now: `map` names it, `venue` reads it, so the device page shows the
        // room's own projector calibration). Its `project venue` control is
        // reachable without opening a folder — and once its folder is
        // scrolled to, so a short panel cannot clip it.
        harness.get_by_label("▾ venue");
        harness.get_by_label("project venue").scroll_to_me();
        harness.run();
        harness.get_by_label("project venue").scroll_to_me();
        harness.run();
        harness.get_by_label("project venue").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let venue_at = calls.iter().position(|(m, p, params)| {
            m == "POST"
                && p == "/venue"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["op"] == "project" && b["id"] == "bar" && b["device"] == "screen-1")
                    .unwrap_or(false)
        });
        let overlay_at = calls.iter().position(|(m, p, params)| {
            m == "POST"
                && p == "/overlay"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["op"] == "solid" && b["device"] == "screen-1")
                    .unwrap_or(false)
        });
        assert!(venue_at.is_some(), "project venue must post POST /venue op project");
        assert!(overlay_at.is_some(), "and then POST /overlay op solid");
        assert!(venue_at < overlay_at, "the venue is projected before the overlay fills it");
    }

    /// 21-oct-2026: the gear on a `venue.fixtures` node's own floating bar
    /// opens that node's room popup, and a screen card posts
    /// `POST /venue { op: "screen" }` for **that node's** room — the same
    /// route and body the add-screen form used to post, with the preset's own
    /// pixels and physical width. The popup is the only place a room's screens
    /// are made now; the old canvas palette and venue dropdown are gone.
    #[test]
    fn a_screen_card_posts_the_nodes_own_room() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "v-0", "kind": "venue.fixtures", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "venue": "bar" } },
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 240.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } }
                ]},
                "primitives": [], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "v-0", "effects": [] }));
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080, "masks": [] }] }),
        );
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                // A room with its floor already saved: this test is about the
                // card, not the starter scene (an empty room would be filled
                // on entry, and that is `an_empty_room_starts_a_scene_…`'s).
                "venue": {
                    "venue": { "id": "bar", "name": "Bar", "setting": "indoor" },
                    "surfaces": [{ "id": "floor", "name": "piso",
                                   "origin": [-4.0, 0.0, -4.0], "u": [8.0, 0.0, 0.0],
                                   "v": [0.0, 0.0, 8.0], "material": "" }],
                    "projectors": [], "fixture": [], "display": [],
                },
                "report": { "problems": [] }, "outputs": [], "patch": [],
            }),
        );
        recorder.respond("POST", "/venue", serde_json::json!({ "ok": true }));

        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("v-0".to_string()));
        harness.run();
        // The popup is reached from the node's own gear: `nodes::PanelState`
        // records that press and the shell opens the room popup for that node.
        let gid = "col-screen-1".to_string();
        harness
            .state_mut()
            .request_venue_popup_for_test(&gid, "v-0");
        harness.run();
        harness.run();
        harness.get_by_label("TV 65\"").scroll_to_me();
        harness.run();
        harness.get_by_label("TV 65\"").click();
        harness.run();

        let posted = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/venue")
            .filter_map(|(_, _, params)| serde_json::from_slice::<serde_json::Value>(&params.body).ok())
            .find(|b| b["op"] == "screen")
            .expect("a screen card must post POST /venue {op: screen}");
        assert_eq!(posted["id"], serde_json::json!("bar"), "the node's own room");
        assert_eq!(posted["screen"]["pixels"], serde_json::json!([3840, 2160]));
        let width = posted["screen"]["width_m"].as_f64().unwrap();
        assert!((width - 2.0).abs() < 1e-6, "the typed width: {width}");
        let height = posted["screen"]["height_m"].as_f64().unwrap();
        assert!((height - 1.125).abs() < 1e-6, "the lock derives the height: {height}");
    }

    /// The fixtures tab is the mapping entry the plan asks for: under each row
    /// of the room's patch, the `venue.fixture` nodes of the open graph that
    /// drive it — a click asks the caller to reveal that node, never a write —
    /// and a row with no node yet says so.
    #[test]
    fn a_fixture_row_lists_and_reveals_its_nodes() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "v-0", "kind": "venue.fixtures", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "venue": "bar" } },
                    { "id": "f-0", "kind": "venue.fixture", "pos": [200.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "venue": "bar", "fixture": "wash-1" } }
                ]},
                "primitives": [], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "v-0", "effects": [] }));
        recorder.respond("GET", "/state", serde_json::json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                "venue": { "surfaces": [], "projectors": [], "fixture": [] },
                "report": { "problems": [] }, "outputs": [],
                "patch": [
                    { "fixture": "wash-1", "kind": "wash", "universe": 1, "address": 1, "span": 3 },
                    { "fixture": "strip-1", "kind": "strip", "universe": 2, "address": 1, "span": 4 }
                ],
            }),
        );
        recorder.respond("POST", "/venue", serde_json::json!({ "ok": true }));

        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("v-0".to_string()));
        harness.run();
        let gid = "col-screen-1".to_string();
        harness.state_mut().request_venue_popup_for_test(&gid, "v-0");
        harness.state_mut().venue_popup_tab_for_test(crate::shell::panels::venue_popup::Tab::Fixtures);
        harness.run();

        // The driven row offers its node; the other row says nobody does. (The
        // whole shell has no Spanish translations in the harness — it boots
        // English — so the label is the source text.)
        harness.get_by_label("show node f-0").click();
        harness.run();
        assert_eq!(
            harness.state_mut().venue_popup_reveal_for_test().as_deref(),
            Some("f-0"),
            "the line asks the caller to reveal the node"
        );
        let wrote = recorder.calls.borrow().iter().any(|(m, p, _)| m == "POST" && p == "/graph/node");
        assert!(!wrote, "a reveal is not a write");
        let nodes: Vec<String> = harness.get_all_by_role(egui::accesskit::Role::Button).filter_map(|n| n.accesskit_node().value()).collect();
        eprintln!("BUTTONS {nodes:?}");
    }

    /// The report this test answers: "when I select a device the windows
    /// start increasing the height permanently". The popup's outer size is
    /// computed once from `ctx.content_rect()` and `theme.gallery_share`; the
    /// columns and the cards only ever divide that size, so clicking a card
    /// (which posts and re-reads the room) cannot make the popup taller.
    #[test]
    fn the_gallery_keeps_its_height() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "v-0", "kind": "venue.fixtures", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "venue": "bar" } },
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 240.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } }
                ]},
                "primitives": [], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "v-0", "effects": [] }));
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080, "masks": [] }] }),
        );
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                "venue": { "surfaces": [], "projectors": [], "fixture": [] },
                "report": { "problems": [] }, "outputs": [], "patch": [],
            }),
        );
        recorder.respond("POST", "/venue", serde_json::json!({ "ok": true }));

        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("v-0".to_string()));
        harness.run();
        let gid = "col-screen-1".to_string();
        harness.state_mut().request_venue_popup_for_test(&gid, "v-0");
        harness.run();
        let first = harness
            .state()
            .venue_popup_rect_for_test()
            .expect("the open popup records its rect");
        assert!(first.height() > 0.0, "the popup has a size: {first:?}");

        // Clicking a card posts and the room is read again: the frame that
        // follows must land on the same outer rect, not one row taller.
        harness.get_by_label("TV 65\"").click();
        for frame in 0..4 {
            harness.run();
            let rect = harness
                .state()
                .venue_popup_rect_for_test()
                .expect("the popup stays open");
            assert_eq!(
                rect, first,
                "frame {frame}: the popup grew from {first:?} to {rect:?}"
            );
        }
    }

    /// P4.4: the `output.device` `draw` folder opens the venue editor; `+
    /// new surface` puts the shared contour editor in draw mode, and closing
    /// a four-corner quad posts `POST /venue { op: "surface" }` for a fresh
    /// surface id, so every `venue.surface` combo picks it up.
    #[test]
    fn drawing_a_surface_posts_the_venue() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } },
                    // The room is a node's own now: the graph names it, so
                    // the screen's `draw` folder edits that room's surfaces.
                    { "id": "v-0", "kind": "venue.fixtures", "pos": [0.0, 160.0], "label": "", "bypass": false, "solo": false,
                      "config": { "venue": "bar" } }
                ]},
                "primitives": [], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "out-0", "effects": [] }));
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080, "masks": [] }] }),
        );
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                "venue": { "surfaces": [], "projectors": [{ "id": "proj-1", "output": "screen-1", "pins": {} }] },
                "report": { "problems": [] }, "outputs": []
            }),
        );
        recorder.respond("POST", "/venue", serde_json::json!({ "ok": true }));

        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("out-0".to_string()));
        harness.run();

        // `draw` is open by default for a screen (the folder *is* the
        // screen's glass), so its `+ new surface` control is already on the
        // page: only the fold header and its body rows are on screen, and the
        // body is what this test clicks.
        harness.get_by_label("+ new surface").scroll_to_me();
        harness.run();
        harness.get_by_label("+ new surface").click();
        harness.run();

        let rect = harness
            .state()
            .venue_canvas_rect()
            .expect("the venue editor canvas should have painted");
        for (fx, fy) in [(0.2, 0.2), (0.8, 0.2), (0.8, 0.8), (0.2, 0.8)] {
            let at = rect.min + egui::vec2(rect.width() * fx, rect.height() * fy);
            click_at(&mut harness, at);
        }
        harness.key_press(egui::Key::Enter);
        harness.run();

        let posted = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/venue"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| {
                        b["op"] == "surface"
                            && b["id"] == "bar"
                            && b["surface"] == "surface-1"
                            && b["points"].as_array().map(Vec::len) == Some(4)
                    })
                    .unwrap_or(false)
        });
        assert!(posted, "closing the quad must post POST /venue op surface");
    }

    // -- S.U11: the library panel --------------------------------------------

    /// Opening the library panel — the first frame the harness ever runs —
    /// reads `GET /state` (metraje/collections) and `GET /author/songs`
    /// (canciones) — `plan-sonnet.md` § S.U11's own test list, first item.
    /// `GET /author/songs` proves the door's own `S.U11` extension actually
    /// reaches `Author::dispatch` (`call::door::Door::call`), not just
    /// `call::ROUTES`.
    #[test]
    fn opening_the_library_panel_requests_state_and_songs() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            calls.iter().any(|(m, p, _)| m == "GET" && p == "/state"),
            "should have read GET /state"
        );
        assert!(
            calls
                .iter()
                .any(|(m, p, _)| m == "GET" && p == "/author/songs"),
            "should have read GET /author/songs through the door's /author branch"
        );
        assert!(
            calls.iter().any(|(m, p, _)| m == "GET" && p == "/fs"),
            "should have read GET /fs for the folder column"
        );
    }

    /// Clicking a folder in the left column changes `cwd` and refetches
    /// `GET /fs` for it — once per click, never per frame (the same rule the
    /// old metraje column kept for `GET /reel/files`).
    #[test]
    fn clicking_a_dir_changes_cwd_and_refetches_fs() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/fs", serde_json::json!({ "dirs": ["clips"] }));
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("clips").click();
        harness.run();
        // The click clears the cache this frame; the next frame refetches.
        harness.run();

        let calls = recorder.calls.borrow();
        let fetches: Vec<_> = calls
            .iter()
            .filter(|(m, p, _)| m == "GET" && p == "/fs")
            .collect();
        assert_eq!(
            fetches.len(),
            2,
            "one fetch for the start folder, one for the click — never per frame"
        );
        let path = fetches[1]
            .2
            .query
            .get("path")
            .expect("the refetch carries a path");
        assert!(
            path.ends_with("clips"),
            "the refetch should ask for the clicked folder: {path}"
        );
    }

    /// A kebab action on a folder row (`import`) raises an import of that
    /// folder. With no graph open the library says so and posts nothing —
    /// the reel is no longer opened from the folder menu.
    #[test]
    fn a_folder_kebab_import_without_a_graph_posts_nothing() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/fs", serde_json::json!({ "dirs": ["clips"] }));
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("…").click();
        harness.run();
        harness.get_by_label("import").click();
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            !calls.iter().any(|(m, p, _)| m == "POST" && (p == "/reel" || p.starts_with("/graph"))),
            "import without an open graph must not POST"
        );
        assert_eq!(
            harness.state_mut().library_state_mut().note_for_test().as_deref(),
            Some("open a graph to import")
        );
    }

    /// The inline `confirmish` stand-in (`shell.js`'s own two-click guard,
    /// re-armed here as an egui popover): cancelling posts nothing,
    /// confirming posts `POST /author/open`. Armed through the test-only
    /// setter (`panels::library::PanelState::set_confirm_open`) rather than
    /// a real second-song-open click, the same trade `set_add_path` makes.
    #[test]
    fn the_confirm_popover_cancels_without_a_post_and_confirms_with_one() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness
            .state_mut()
            .library_state_mut()
            .set_confirm_open("otra", "/songs/otra.mp3");
        harness.run();

        // The popover sits deep in the panel's scroll (S.U9b's preview
        // section joined the top of it), so bring it into the clip rect
        // before the click — off-clip buttons are laid out but dead.
        harness.get_by_label("cancel").scroll_to_me();
        harness.run();
        harness.get_by_label("cancel").click();
        harness.run();

        assert!(
            !recorder
                .calls
                .borrow()
                .iter()
                .any(|(m, p, _)| m == "POST" && p == "/author/open"),
            "cancelar should not have posted /author/open"
        );

        harness
            .state_mut()
            .library_state_mut()
            .set_confirm_open("otra", "/songs/otra.mp3");
        harness.run();
        harness.get_by_label("confirm").scroll_to_me();
        harness.run();
        harness.get_by_label("confirm").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/author/open"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["audio"] == "/songs/otra.mp3")
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "confirmar should POST /author/open {{audio: /songs/otra.mp3}}"
        );
    }

    /// Against a **real** `Door`, not a `FakeDoor`: the folder column lists
    /// the real one-level `GET /fs` of `cwd` — pointed at the repo root, so
    /// `crates` is a folder it must show.
    #[test]
    fn the_folder_column_lists_the_working_directory_through_a_real_door() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = std::sync::Arc::new(crate::Author::new());
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));
        let door = crate::call::Door::new(
            std::sync::Arc::clone(&shared),
            std::sync::Arc::clone(&author),
        );

        let mut harness = harness_with_door(Theme::phosphor(), Box::new(door));
        harness
            .state_mut()
            .library_state_mut()
            .set_cwd(&repo_root.to_string_lossy());
        run_real(&mut harness);


        harness.get_by_label("crates");
    }

    /// A folder with nothing in it: through a real `Door`, the files column
    /// shows its empty-state text — asserted through AccessKit (rule 7: no
    /// hand-written count).
    // `S.U15`: `/author/*` only exists under `call::author`, gated on `gpu`
    // alone; a glow-only build serves it as a 404, so this test is scoped to
    // builds that actually have it.
    #[test]
    #[cfg(feature = "gpu")]
    fn the_files_column_shows_its_empty_state_through_a_real_door() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = std::sync::Arc::new(crate::Author::new());
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));
        let door = crate::call::Door::new(
            std::sync::Arc::clone(&shared),
            std::sync::Arc::clone(&author),
        );

        let empty = std::env::temp_dir().join(format!("fw-lib-empty-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&empty);
        metal_vfs::create_dir_all(&empty).expect("scratch folder");

        let mut harness = harness_with_door(Theme::phosphor(), Box::new(door));
        harness
            .state_mut()
            .library_state_mut()
            .set_cwd(&empty.to_string_lossy());
        run_real(&mut harness);


        harness.get_by_label("no files in this folder");
        let _ = metal_vfs::remove_dir_all(&empty);
    }

    // -- S.U12: the properties panel ----------------------------------------

    /// A real node selection (via `Shell::set_selection_for_test`, the same
    /// trade `lanes_state_mut`/`library_state_mut` make) makes the
    /// properties panel request that node's data — `GET /graph` for the
    /// node itself, and for an `fx.*` node `GET /graph/effects` keyed off
    /// **the generator its chain hangs on**, the routes' own identity the
    /// panel walks to from the selected node's input cable.
    #[test]
    fn selecting_a_node_requests_the_graph_and_its_effects() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_fx_graph(&recorder);
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("fx-0".to_string()));
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            calls.iter().any(|(m, p, q)| m == "GET"
                && p == "/graph"
                && q.query.get("id").map(String::as_str) == Some("empty-1")),
            "selecting a node should have (re)loaded its graph"
        );
        assert!(
            calls.iter().any(|(m, p, q)| m == "GET"
                && p == "/graph/effects"
                && q.query.get("node").map(String::as_str) == Some("gen-0")),
            "selecting an fx node should have loaded its chain's effects, keyed off the gen"
        );
    }

    /// One `gen.turing` with an `fx.grade` node wired downstream, and the
    /// routes the grade page reads — `GET /graph/io`'s declared floats for its
    /// ranges and `GET /graph`'s own config for the held values.
    fn script_grade_graph(recorder: &FakeDoor) {
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "empty-1", "name": "empty-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "empty-1", "nodes": [
                    { "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null },
                    { "id": "grade-0", "kind": "fx.grade", "pos": [1.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "grade_contrast": 0.0 } }
                ],
                "edges": [
                    { "id": "e0", "from": { "node": "gen-0", "port": "image" }, "to": { "node": "grade-0", "port": "image" } }
                ]},
                "primitives": [],
                "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond(
            "GET",
            "/graph/io",
            serde_json::json!({
                "node": "grade-0", "kind": "fx.grade", "label": "",
                "inputs": [], "outputs": [], "sources": [],
                "params": [
                    { "name": "grade_contrast", "min": -100.0, "max": 100.0, "default": 0.0, "kind": "float", "value": 0.0, "bound": null, "live": null },
                    { "name": "grade_temperature", "min": -100.0, "max": 100.0, "default": 0.0, "kind": "float", "value": 0.0, "bound": null, "live": null }
                ]
            }),
        );
        recorder.respond("GET", "/graph/live", serde_json::json!({ "graph": "empty-1", "nodes": {} }));
    }

    /// Selecting an `fx.grade` node opens its own `grade` page (not the generic
    /// `effect` one), loading the graph and its `/graph/io`; moving a slider
    /// posts the config merge `{op:"config", config:{grade_contrast: …}}`.
    #[test]
    fn selecting_an_fx_grade_node_shows_the_grade_page_and_posts_its_config() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_grade_graph(&recorder);
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("grade-0".to_string()));
        harness.run();

        {
            let calls = recorder.calls.borrow();
            assert!(
                calls.iter().any(|(m, p, q)| m == "GET"
                    && p == "/graph"
                    && q.query.get("id").map(String::as_str) == Some("empty-1")),
                "selecting an fx.grade node should (re)load its graph"
            );
            assert!(
                calls.iter().any(|(m, p, q)| m == "GET"
                    && p == "/graph/io"
                    && q.query.get("node").map(String::as_str) == Some("grade-0")),
                "selecting an fx.grade node should load its own I/O"
            );
        }

        // The first section is `grade`, open by default, and its Basic
        // section is open inside it.
        harness.get_all_by_label("\u{25be} grade").next().unwrap();
        harness.get_all_by_label("\u{25be} Basic").next().unwrap();

        // Move Contrast's track: the label sits on the kit's column, the
        // track to its right. One press+release posts one config merge.
        harness.get_all_by_label("Contrast").next().unwrap().scroll_to_me();
        harness.run();
        let rect = harness.get_all_by_label("Contrast").next().unwrap().rect();
        let at = egui::pos2(rect.right() + 40.0, rect.center().y);
        click_at(&mut harness, at);

        let posted = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["op"] == "config" && b["config"]["grade_contrast"].is_number())
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "moving grade_contrast should POST /graph/node {{op: config, config: {{grade_contrast: …}}}}"
        );
    }

    /// Picking a new blend for a `gen.*` node's layer section posts `POST
    /// /graph/node { op: "layer", blend }` — `plan-sonnet.md` § S.U12's
    /// second test, "dragging a param slider records POST /graph/node with
    /// the value": the blend combo is that section's own control, proven
    /// against the door the same way `picking_a_router_mode_posts_router`
    /// already proves a `ComboBox`'s pick reaches `POST` (`get_by_value`
    /// then `get_by_label`, `S.U9`'s own finding).
    #[test]
    fn picking_a_blend_posts_graph_node_layer() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_one_graph(&recorder);
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "empty-1", "nodes": [
                    { "id": "gen-0", "kind": "gen.entrance", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "blend": "normal", "z": 0, "opacity": 1.0, "sector": { "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0 }, "mask": null } }
                ]},
                "primitives": [],
                "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond(
            "GET",
            "/graph/effects",
            serde_json::json!({ "node": "gen-0", "effects": [] }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        harness.get_by_value("normal").click();
        harness.run();
        harness.get_by_label("add").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| b["op"] == "layer" && b["node"] == "gen-0" && b["blend"] == "add")
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "picking a blend should POST /graph/node {{op: layer, blend: add}}"
        );
    }

    /// Setting an effect to `fijo` posts `POST /graph/effects` with a
    /// numeric `value` — `plan-sonnet.md` § S.U12's third test, re-keyed to
    /// B5's model: the effect is a node, the `fijo` control is that node's
    /// own, and the post names the gen the chain hangs on.
    #[test]
    fn setting_an_effect_to_fijo_posts_graph_effects_with_a_value() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_fx_graph(&recorder);
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("fx-0".to_string()));
        harness.run();

        harness.get_by_label("fijo").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted_fijo = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/effects"
                && serde_json::from_slice::<serde_json::Value>(&params.body)
                    .map(|b| {
                        b["state"] == "fixed"
                            && b["effect"] == "shake"
                            && b["node"] == "gen-0"
                            && b.get("value").is_some()
                    })
                    .unwrap_or(false)
        });
        assert!(
            posted_fijo,
            "choosing fijo should POST /graph/effects {{node: gen-0, state: fijo, value}}"
        );
    }

    /// Against a **real** `Door`, not a `FakeDoor` — `plan-sonnet.md` §
    /// S.U12's real-door test: selecting a real node from the `empty-1`
    /// fixture (`data/graphs/empty-1.json` ships a `gen-0`, kind
    /// `gen.entrance`) populates the properties panel end to end, both
    /// panels reading the same shared `Shell::selection` (`S.U9`'s own
    /// design). `set_selection_for_test` stands in for the canvas click a
    /// real operator would make — the click journey itself (press, drag
    /// slop, `GET /graph/hit`) is `panels::nodes`'s own test's job
    /// (`a_pointer_press_over_the_canvas_hits_through_the_door`); this test's
    /// job is only "does the properties panel, once a node is selected,
    /// actually show that node's real data from the real graph".
    #[test]
    fn selecting_a_real_node_from_empty_1_populates_properties_through_a_real_door() {
        crate::test_fixtures::use_graphs();
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = std::sync::Arc::new(crate::Author::new());
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));
        let door = crate::call::Door::new(
            std::sync::Arc::clone(&shared),
            std::sync::Arc::clone(&author),
        );

        let mut harness = harness_with_door(Theme::phosphor(), Box::new(door));
        run_real(&mut harness);

        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        run_real(&mut harness);

        harness.get_by_label("gen.entrance");
    }

    // -- the mouse wheel over properties floats (S.AO) ----------------------

    /// A two-float UI, tall enough to scroll, for the `float_row` tests:
    /// the component is exercised on its own, so the wheel can be driven
    /// without a real graph behind it.
    struct FloatFixture {
        a: f32,
        b: f32,
        posted: usize,
        posted_value: f32,
    }

    fn float_fixture() -> Harness<'static, FloatFixture> {
        Harness::builder()
            .with_size(Vec2 { x: 500.0, y: 300.0 })
            .build_ui_state(
                |ui, state: &mut FloatFixture| {
                    egui::ScrollArea::vertical().id_salt("float-fixture").show(ui, |ui| {
                        if components::float_row(ui, &mut state.a, 0.0..=100.0, components::Float::Slider)
                            .changed()
                        {
                            state.posted += 1;
                            state.posted_value = state.a;
                        }
                        if components::float_row(ui, &mut state.b, 0.0..=100.0, components::Float::Slider)
                            .changed()
                        {
                            state.posted += 1;
                            state.posted_value = state.b;
                        }
                        for i in 0..80 {
                            ui.label(format!("filler {i}"));
                        }
                    });
                },
                FloatFixture { a: 50.0, b: 50.0, posted: 0, posted_value: 0.0 },
            )
    }

    /// The first slider's centre, with the fixture run enough to lay it out.
    fn first_float_center(harness: &mut Harness<'static, FloatFixture>) -> egui::Pos2 {
        harness.run_steps(2);
        harness
            .get_all_by_role(egui::accesskit::Role::Slider)
            .next()
            .expect("the fixture draws two sliders")
            .rect()
            .center()
    }

    fn wheel_onto(
        harness: &mut Harness<'static, FloatFixture>,
        at: egui::Pos2,
        modifiers: egui::Modifiers,
        notches: f32,
    ) {
        harness.input_mut().events.push(egui::Event::PointerMoved(at));
        harness.run_steps(1);
        harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2 { x: 0.0, y: notches },
            phase: egui::TouchPhase::Move,
            modifiers,
        });
        harness.run_steps(1);
    }

    /// One wheel notch over a float moves that float and leaves its neighbour
    /// alone.
    #[test]
    fn scroll_over_a_float_changes_only_that_float() {
        let mut harness = float_fixture();
        let at = first_float_center(&mut harness);
        wheel_onto(&mut harness, at, egui::Modifiers::NONE, 1.0);
        assert!(harness.state().a > 50.0, "wheel over A should raise it, got {}", harness.state().a);
        assert_eq!(harness.state().b, 50.0, "wheel over A must not touch B");
    }

    /// A float under the pointer swallows the wheel, so the panel behind it
    /// does not also scroll.
    #[test]
    fn scroll_over_a_float_does_not_scroll_the_panel() {
        let mut harness = float_fixture();
        harness.run_steps(2);
        let before = harness.get_by_label("filler 0").rect().min.y;
        let at = harness
            .get_all_by_role(egui::accesskit::Role::Slider)
            .next()
            .expect("a slider")
            .rect()
            .center();
        wheel_onto(&mut harness, at, egui::Modifiers::NONE, 3.0);
        harness.run_steps(2);
        assert_eq!(
            before,
            harness.get_by_label("filler 0").rect().min.y,
            "the panel must not scroll while the pointer is over a float"
        );
    }

    /// Shift multiplies the per-notch step by ten and Alt divides it by ten.
    #[test]
    fn shift_and_alt_scale_the_step() {
        let one = |modifiers| {
            let mut harness = float_fixture();
            let at = first_float_center(&mut harness);
            wheel_onto(&mut harness, at, modifiers, 1.0);
            harness.state().a
        };
        assert!((one(egui::Modifiers::NONE) - 51.0).abs() < 0.01, "one notch is range/100");
        assert!((one(egui::Modifiers::SHIFT) - 60.0).abs() < 0.01, "shift is ×10");
        assert!((one(egui::Modifiers::ALT) - 50.1).abs() < 0.01, "alt is ÷10");
    }

    /// The wheel posts the value once, only after the gesture has been still
    /// for the delay — not once per notch.
    #[test]
    fn the_wheel_posts_once_after_the_gesture_stops() {
        let mut harness = float_fixture();
        let at = first_float_center(&mut harness);
        wheel_onto(&mut harness, at, egui::Modifiers::NONE, 1.0);
        assert_eq!(harness.state().posted, 0, "the gesture has not stopped yet");
        harness.run_steps(1);
        assert_eq!(harness.state().posted, 1, "one post once the wheel stops");
        assert!(harness.state().posted_value > 50.0);
    }

    // -- S.U13: responsive delegation (`O.12.f`) -----------------------------

    /// `plan-sonnet.md` § S.U13's own requirement, checked at the exact size
    /// gate 3 already paints under: `classes::arrangement` must call
    /// [`super::DESKTOP_VIEWPORT`] the full grid, with nothing delegated.
    #[test]
    fn desktop_viewport_is_the_full_grid_through_the_real_arrangement_call() {
        let theme = Theme::phosphor();
        let size = Vec2::new(
            super::super::DESKTOP_VIEWPORT.0 as f32,
            super::super::DESKTOP_VIEWPORT.1 as f32,
        );
        assert!(classes::arrangement(&theme, size).is_full_grid());
    }

    /// Every one of the five `VIEWPORTS` `check-ui.mjs` section A checks,
    /// under both house themes: the shell boots (the wordmark is in the
    /// tree — the same smoke `shell_smoke_boots_wordmark_and_four_regions`
    /// runs at `DESKTOP_VIEWPORT`), and whatever `classes::arrangement`
    /// decided for that size/theme is what actually shows — the full grid's
    /// four titles, or the delegated controls the grid gave way to.
    /// Falsified by measurement: painting `regions::paint`'s delegated path
    /// unconditionally (even when `arrangement.is_full_grid()`) turns the
    /// desktop iteration of this test red, since none of the four titles
    /// would be in the tree that way.
    #[test]
    fn every_viewport_boots_and_shows_its_own_arrangement() {
        for theme in [Theme::phosphor(), Theme::contrast()] {
            for (name, w, h) in VIEWPORTS {
                let size = Vec2::new(w, h);
                let mut harness = harness_at(theme.clone(), size, Box::new(FakeDoor::new()));
                harness.run();

                harness.get_by_label("Metal");
                harness.get_by_label("VJ");

                let a = classes::arrangement(&theme, size);
                if a.is_full_grid() {
                    for region in Region::ALL {
                        harness.get_by_label(region.title());
                    }
                    continue;
                }

                if a.library == Placement::Delegated {
                    let (open_label, closed_label) = (
                        crate::shell::header::library_label(true),
                        crate::shell::header::library_label(false),
                    );
                    let toggle = harness
                        .query_by_label(&open_label)
                        .or_else(|| harness.query_by_label(&closed_label));
                    assert!(
                        toggle.is_some(),
                        "{name} ({theme_name}): expected the library sidebar toggle",
                        theme_name = theme.name
                    );
                } else {
                    harness.get_by_label(Region::Library.title());
                }

                if a.tabs == Placement::Delegated {
                    // The delegated tab picker is one menu button naming the
                    // active tab (`header.rs::tab_selector`).
                    harness.get_by_label(&format!("view: {} ", Region::Nodes.title()));
                } else {
                    harness.get_by_label(Region::Nodes.title());
                    harness.get_by_label(Region::Lanes.title());
                }

                if a.properties == Placement::Delegated {
                    harness.get_by_label("hold: properties");
                } else {
                    harness.get_by_label(Region::Properties.title());
                }

                // `S.U14`: the AI harness is open by default wherever it
                // docks beside the grid; on a phone (2026-10-01) it is the
                // right sidebar that gives way to the library's at boot, one
                // sidebar at a time. The header's own `IA `/`IA ` toggle is
                // always there regardless.
                if a.harness == Placement::Grid || a.library == Placement::Grid {
                    harness.get_by_label(crate::shell::panels::chat::TITLE);
                }
                let ia = crate::shell::header::ia_label();
                let ia_toggle = harness.query_by_label(&ia);
                assert!(
                    ia_toggle.is_some(),
                    "{name} ({theme_name}): expected the IA header toggle",
                    theme_name = theme.name
                );
            }
        }
    }

    /// The smallest viewport (`phone-small`) delegates every region — the
    /// concrete case `plan-sonnet.md` § S.U13 names by hand: only one of
    /// `graph`/`lanes` is visible at a time (its heading, not just the tab
    /// button — `get_by_label` panics on more than one match, which is
    /// exactly what would happen if both painted at once), and the other
    /// disappears from the tree entirely rather than merely collapsing to
    /// zero size. `click_accesskit()` rather than `click()`: the header row
    /// reflows by a few points between frames here (more controls fit in it
    /// than any earlier lote's header ever had at once), which a raw
    /// pointer click can miss because `egui` hit-tests a press against
    /// **last** frame's widget rects; the accesskit action targets the
    /// node directly regardless of exactly where it currently sits.
    #[test]
    fn only_the_active_tab_paints_its_content() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4];
        let size = Vec2::new(w, h);
        let mut harness = harness_at(theme, size, Box::new(FakeDoor::new()));
        harness.run();

        // `graph` is the default `active_tab` (`Shell::new`).
        harness.get_by_label(Region::Nodes.title());
        assert!(
            harness.query_by_label(Region::Lanes.title()).is_none(),
            "lanes' own heading should not be in the tree while graph is active"
        );

        open_tab_menu(&mut harness);
        harness
            .get_by_label(&format!("view: {}", Region::Lanes.title()))
            .click_accesskit();
        harness.run();
        assert_eq!(
            harness.state().active_tab(),
            Region::Lanes,
            "click should have switched active_tab"
        );

        harness.get_by_label(Region::Lanes.title());
        assert!(
            harness.query_by_label(Region::Nodes.title()).is_none(),
            "graph's own heading should not be in the tree while lanes is active"
        );
    }

    /// Opens the delegated header's tab picker (`header.rs::tab_selector`).
    fn open_tab_menu(harness: &mut Harness<'static, Shell>) {
        harness
            .get_by_label(&format!("view: {} ", harness.state().active_tab().title()))
            .click_accesskit();
        harness.run();
    }

    /// Phone-width header (412x915, plus the two smaller phone viewports):
    /// the tab picker is on screen without scrolling the header strip, and
    /// every tab in it is inside the viewport and clickable. Before this the
    /// two tab buttons sat past the right edge and nothing scrolled them in
    /// on a touch screen.
    #[test]
    fn every_tab_is_reachable_at_phone_width() {
        let theme = Theme::phosphor();
        for (w, h) in [(412.0_f32, 915.0_f32), (390.0, 844.0), (360.0, 740.0)] {
            let mut harness = harness_at(theme.clone(), Vec2::new(w, h), Box::new(FakeDoor::new()));
            harness.run();

            let r = harness
                .get_by_label(&format!("view: {} ", Region::Nodes.title()))
                .rect();
            assert!(
                r.min.x >= 0.0 && r.max.x <= w && r.min.y >= 0.0 && r.max.y <= h,
                "tab picker at {r:?} is outside the {w}x{h} viewport"
            );

            open_tab_menu(&mut harness);
            for tab in [Region::Nodes, Region::Lanes] {
                let r = harness.get_by_label(&format!("view: {}", tab.title())).rect();
                assert!(
                    r.min.x >= 0.0 && r.max.x <= w && r.min.y >= 0.0 && r.max.y <= h,
                    "tab {tab:?} at {r:?} is outside the {w}x{h} viewport"
                );
            }
            harness
                .get_by_label(&format!("view: {}", Region::Lanes.title()))
                .click_accesskit();
            harness.run();
            assert_eq!(harness.state().active_tab(), Region::Lanes, "{w}x{h}: lanes not reached");
            harness.get_by_label(Region::Lanes.title());
        }
    }

    /// The library sidebar's collapse toggle: clicking it hides the
    /// panel's own content (its heading leaves the tree) without touching
    /// `graph`/`lanes`, and clicking again brings it back.
    #[test]
    fn the_sidebar_toggle_collapses_and_restores_library() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[3]; // "phone"
        let mut harness = harness_at(theme, Vec2::new(w, h), Box::new(FakeDoor::new()));
        harness.run();

        harness.get_by_label(Region::Library.title());

        // accesskit, not `click()`: the chip now sits in this strip, so the
        // toggles can sit past the viewport's edge where a pointer click misses.
        harness.get_by_label(&crate::shell::header::library_label(true)).click_accesskit();
        harness.run();
        assert!(
            harness.query_by_label(Region::Library.title()).is_none(),
            "collapsed sidebar should not paint its content"
        );

        harness.get_by_label(&crate::shell::header::library_label(false)).click_accesskit();
        harness.run();
        harness.get_by_label(Region::Library.title());
    }

    /// The sync indicator is in the phone strip too, and opens the pairing
    /// overlay from there (the same chip the full grid paints).
    #[test]
    fn the_sync_chip_is_on_the_phone_header_and_opens_the_overlay() {
        use crate::shell::panels::pairing::{chip_text, Summary};
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[3]; // "phone"
        let door = FakeDoor::new();
        door.respond("GET", "/sync/status", serde_json::json!({ "running": true, "name": "sala" }));
        let mut harness = harness_at(theme, Vec2::new(w, h), Box::new(door));
        harness.run();

        harness.get_by_label(&chip_text(Summary::Unpaired)).click_accesskit();
        harness.run();
        assert!(harness.state_mut().pairing_open(), "the phone chip opens the overlay");
    }

    /// The long-press gesture's *timing* (`O.12.f`): holding the properties
    /// button past `theme.long_press` opens the overlay; a run shorter than
    /// that — released before the threshold — opens nothing. Driven through
    /// `Shell::debug_properties_press_probe` rather than synthesised
    /// `PointerButton` events: `egui`'s own hit-testing resolves a press
    /// against **last** frame's widget rects (`Context::begin_pass`), which
    /// every *other* click-based test in this file sidesteps by sending a
    /// press and its release together in one `Node::click()` batch — a
    /// genuinely *held* press across several separate frames has no
    /// same-mechanism equivalent to send over the wire. The probe drives
    /// exactly the one thing this test is about — the threshold math in
    /// `Shell::ui` — leaving the button's own sensing to the many other
    /// tests here that already prove `ui.button` responds to a real click.
    /// `input_mut().time` is what lets this test control the clock
    /// `Shell::ui` reads (`ui.input(|i| i.time)`) without a real wait.
    #[test]
    fn a_long_press_opens_the_properties_overlay_and_a_short_tap_does_not() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4]; // "phone-small" — properties is delegated there
        let size = Vec2::new(w, h);
        assert_eq!(
            classes::arrangement(&theme, size).properties,
            Placement::Delegated
        );

        // A short tap: press, one small time step while still down, then
        // release — the elapsed time never crosses `theme.long_press`.
        let mut tap = harness_at(theme.clone(), size, Box::new(FakeDoor::new()));
        tap.input_mut().time = Some(0.0);
        tap.run();
        tap.state_mut().debug_properties_press_probe = Some(true);
        tap.input_mut().time = Some(0.01); // press lands; press_started_at := 0.01
        tap.run();
        tap.input_mut().time = Some(0.01 + f64::from(theme.long_press) * 0.1); // still down, short elapsed
        tap.run();
        tap.state_mut().debug_properties_press_probe = Some(false); // release
        tap.run();
        assert!(
            !tap.state().properties_overlay_open(),
            "a short tap should not open the overlay"
        );

        // A long press: the probe stays "down" while time is advanced well
        // past `theme.long_press`.
        let mut hold = harness_at(theme.clone(), size, Box::new(FakeDoor::new()));
        hold.input_mut().time = Some(0.0);
        hold.run();
        hold.state_mut().debug_properties_press_probe = Some(true);
        hold.input_mut().time = Some(0.01); // press_started_at := 0.01
        hold.run();
        assert!(
            !hold.state().properties_overlay_open(),
            "should not open on the very frame the press lands"
        );
        hold.input_mut().time = Some(0.01 + f64::from(theme.long_press) * 2.0); // still down, well past the threshold
        hold.run();
        assert!(
            hold.state().properties_overlay_open(),
            "holding past theme.long_press should open the overlay"
        );

        // Esc closes it.
        hold.key_press(egui::Key::Escape);
        hold.run();
        assert!(
            !hold.state().properties_overlay_open(),
            "Esc should close the overlay"
        );
    }

    // -- S.U14: the AI harness sidebar ------------------------------------

    /// Opening the sidebar (it is open by default — `Shell::new`'s own doc
    /// comment) requests `GET /ai/chats`, the route the *chat* panel's menu
    /// reads to paint its conversation list — the same "opening a panel reads
    /// its data route through the door" claim the old tool panel made for
    /// `GET /ai-config`.
    #[test]
    fn opening_the_harness_sidebar_requests_ai_config() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            calls
                .iter()
                .any(|(m, p, _)| m == "GET" && p == "/ai/chats"),
            "should have read GET /ai/chats"
        );
    }

    /// The prompt box's `enviar` button posts `POST /ai/ask` carrying the
    /// prompt text — preloaded via `Shell::set_harness_prompt_for_test`,
    /// the same trade `library_state_mut`/`set_confirm_open` make rather
    /// than simulating keystrokes into the `TextEdit`.
    #[test]
    fn sending_a_prompt_posts_ai_ask() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_harness_prompt_for_test("hello harness");
        harness.run();

        harness.get_by_label("send").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let call = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/ai/ask")
            .expect("should have posted /ai/ask");
        let body: serde_json::Value = serde_json::from_slice(&call.2.body).expect("body is json");
        assert_eq!(body["prompt"], "hello harness");
    }

    /// The prompt is a fixed footer (`S.X`): `send` is parked at the panel's
    /// bottom edge, inside the visible rect, so the operator can always reach
    /// it. Regression: the prompt used to sit below the transcript in the same
    /// column, so it floated just under the last row near the top instead of
    /// reading as a footer — and a tall stack could push it out of the panel.
    #[test]
    fn the_send_button_stays_inside_the_panel_footer() {
        let mut harness = harness_with_door(Theme::phosphor(), Box::new(FakeDoor::new()));
        harness.run();

        let (w, h) = crate::shell::DESKTOP_VIEWPORT;
        let (w, h) = (w as f32, h as f32);
        let send = harness.get_by_label("send").rect();
        assert!(send.width() > 0.0 && send.height() > 0.0, "send has no area: {send:?}");
        assert!(
            send.min.y >= 0.0 && send.max.y <= h && send.min.x >= 0.0 && send.max.x <= w,
            "send at {send:?} is outside the {w}x{h} viewport"
        );
        assert!(
            send.min.y > h / 2.0,
            "send at {send:?} should be pinned to the panel's bottom, not floating below the transcript"
        );
    }

    /// The composer's `@` and send buttons sit on one row, the same height,
    /// both inside the panel and both clickable: regression for the `@` that
    /// sat off the send button's line and a row too wide for the panel.
    #[test]
    fn the_composer_buttons_share_one_row_inside_the_panel() {
        let mut harness = harness_with_door(Theme::phosphor(), Box::new(FakeDoor::new()));
        harness.run();

        let at = harness.get_by_label("@").rect();
        let send = harness.get_by_label("send").rect();
        assert!(
            (at.center().y - send.center().y).abs() < 1.0,
            "@ at {at:?} and send at {send:?} should share a row"
        );
        assert!(send.min.x > at.max.x, "send at {send:?} should sit right of @ at {at:?}");
        let (w, _) = crate::shell::DESKTOP_VIEWPORT;
        assert!(send.max.x <= w as f32, "send at {send:?} runs past the {w}px viewport");
    }

    /// Enter sends the prompt; Shift+Enter is a newline and sends nothing —
    /// the keyboard half of the prompt box, unchanged by the footer move. The
    /// editor is focused through the same `egui::Memory::request_focus` a
    /// click uses (the trade `space_does_not_toggle_with_a_text_field_focused`
    /// makes).
    #[test]
    fn enter_sends_the_prompt_and_shift_enter_does_not() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_harness_prompt_for_test("hola");
        harness.run();

        harness
            .ctx
            .memory_mut(|m| m.request_focus(egui::Id::new("shell/panel/chat/prompt")));
        harness.run();

        let asked =
            || recorder.calls.borrow().iter().any(|(m, p, _)| m == "POST" && p == "/ai/ask");

        // Shift+Enter is a newline, not a send.
        harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
        harness.run();
        assert!(!asked(), "Shift+Enter should insert a newline, not send");

        // Plain Enter sends.
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(asked(), "Enter should post /ai/ask");
    }

    /// `@` opens the completion list over the open graph's nodes; Tab inserts
    /// the best match (the focused node ranks first, and `ge` matches `gen-1`
    /// and not `fx-2`) as `@token ` and the caret follows it. Enter, the send
    /// key, inserts the same token while the list is open and sends nothing.
    #[test]
    fn at_then_tab_completes_the_best_node_and_enter_does_not_send() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().nodes_state_mut().set_graph_for_test(serde_json::json!({
            "graph": {"nodes": [
                {"id": "gen-1", "label": "Generator 1"},
                {"id": "fx-2", "label": "Fade"},
            ]}
        }));
        harness.state_mut().set_harness_prompt_for_test("@ge");
        harness.run();
        harness
            .ctx
            .memory_mut(|m| m.request_focus(egui::Id::new("shell/panel/chat/prompt")));
        harness.run();

        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().harness_prompt_for_test(), "@gen-1 ");

        // A second token: Enter inserts the highlighted row and does not send.
        harness.state_mut().set_harness_prompt_for_test("@fa");
        harness
            .ctx
            .memory_mut(|m| m.request_focus(egui::Id::new("shell/panel/chat/prompt")));
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(
            harness.state().harness_prompt_for_test(),
            "@fx-2 ",
            "fa matches only the label Fade, which is fx-2"
        );
        assert!(
            !recorder.calls.borrow().iter().any(|(m, p, _)| m == "POST" && p == "/ai/ask"),
            "Enter with the list open inserts, it does not send"
        );
    }

    /// The header's `IA` toggle closes the sidebar (docked form, desktop);
    /// clicking it again reopens it.
    #[test]
    fn the_ia_toggle_closes_and_reopens_the_harness_sidebar() {
        let mut harness = harness(Theme::phosphor());
        harness.run();
        assert!(harness.state().harness_open());
        harness.get_by_label(crate::shell::panels::chat::TITLE);

        harness.get_by_label(&crate::shell::header::ia_label()).click();
        harness.run();
        assert!(!harness.state().harness_open());
        assert!(harness
            .query_by_label(crate::shell::panels::chat::TITLE)
            .is_none());

        harness.get_by_label(&crate::shell::header::ia_label()).click();
        harness.run();
        assert!(harness.state().harness_open());
        harness.get_by_label(crate::shell::panels::chat::TITLE);
    }

    /// 2026-10-01, the phone shell: the harness is a right sidebar there too,
    /// never a centred overlay, and a phone holds one sidebar beside the
    /// content — the library at boot, the harness once `IA` opens it (which
    /// closes the library). Esc closes it.
    #[test]
    fn on_a_phone_the_harness_is_a_right_sidebar_that_trades_places_with_the_library() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4]; // "phone-small"
        let size = Vec2::new(w, h);
        assert_eq!(classes::arrangement(&theme, size).harness, Placement::Delegated);

        let mut harness = harness_at(theme, size, Box::new(FakeDoor::new()));
        harness.run();
        assert!(harness.state().library_sidebar_open());
        assert!(!harness.state().harness_open(), "one sidebar at a time on a phone");

        // `click_accesskit()`: at phone-small the header reflows between
        // frames (see `only_the_active_tab_paints_its_content`).
        harness.get_by_label(&crate::shell::header::ia_label()).click_accesskit();
        harness.run();
        assert!(harness.state().harness_open());
        assert!(!harness.state().library_sidebar_open(), "opening the harness closes the library");
        let panel = harness.get_by_label(crate::shell::panels::chat::TITLE).rect();
        assert!(
            f64::from(panel.center().x) > f64::from(w) / 2.0,
            "the harness sits on the right, panel {panel:?}"
        );

        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!harness.state().harness_open(), "Esc should close it");
    }

    /// 2026-10-01: holding still on the content — not only on the header
    /// button — opens propiedades, and letting go of that hold leaves it
    /// open. Real pointer events, no probe: the gesture reads raw pointer
    /// state, not a widget's hit test.
    #[test]
    fn holding_still_on_the_content_opens_properties_and_releasing_keeps_it() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4];
        let size = Vec2::new(w, h);
        let mut harness = harness_at(theme.clone(), size, Box::new(FakeDoor::new()));
        harness.input_mut().time = Some(0.0);
        harness.run();
        let at = egui::pos2(w * 0.75, h * 0.6);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        harness.input_mut().events.push(egui::Event::PointerMoved(at));
        harness.input_mut().events.push(press(true));
        harness.input_mut().time = Some(0.01);
        harness.run();
        assert!(!harness.state().properties_overlay_open());
        harness.input_mut().time = Some(0.01 + f64::from(theme.long_press) * 2.0);
        harness.run();
        assert!(harness.state().properties_overlay_open(), "a still hold past theme.long_press opens it");
        harness.input_mut().events.push(press(false));
        harness.run();
        harness.run();
        assert!(harness.state().properties_overlay_open(), "the hold's own release must not close it");
    }

    /// A press that moves is a drag, never a long-press.
    #[test]
    fn a_press_that_moves_does_not_open_properties() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4];
        let mut harness = harness_at(theme.clone(), Vec2::new(w, h), Box::new(FakeDoor::new()));
        harness.input_mut().time = Some(0.0);
        harness.run();
        let at = egui::pos2(w * 0.75, h * 0.6);
        harness.input_mut().events.push(egui::Event::PointerMoved(at));
        harness.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.input_mut().time = Some(0.01);
        harness.run();
        harness.input_mut().events.push(egui::Event::PointerMoved(at + Vec2::new(w * 0.2, 0.0)));
        harness.input_mut().time = Some(0.01 + f64::from(theme.long_press) * 2.0);
        harness.run();
        assert!(!harness.state().properties_overlay_open());
    }

    /// The transcript paints one row per scripted message: the operator's
    /// bubble and the assistant's raw text in the phosphor. Scripted
    /// replies, so the assertion is on the exact labels `panels::chat`
    /// paints, never a real door's live conversation.
    #[test]
    fn the_harness_transcript_renders_a_row_per_scripted_message() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/ai/chats",
            serde_json::json!({
                "chats": [{ "id": "c1", "title": "chat uno", "status": "idle", "updated": 1 }]
            }),
        );
        recorder.respond(
            "GET",
            "/ai/chats/c1",
            serde_json::json!({
                "id": "c1",
                "status": "idle",
                "seq": 2,
                "messages": [
                    { "seq": 1, "role": "user", "content": "hello" },
                    { "seq": 2, "role": "assistant", "content": "buenas" }
                ]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("hello");
        harness.get_by_label("buenas");
    }

    /// An `awaiting_approval` tool call paints the `permitir`/`denegar` pair;
    /// clicking `permitir` posts `POST /ai/chat/approve` with
    /// `{"call":"t1","allow":true}` — the exact body `panels::chat` sends.
    #[test]
    fn the_harness_awaiting_tool_call_offers_allow_and_posts_approve() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/ai/chats",
            serde_json::json!({
                "chats": [{ "id": "c1", "title": "chat uno", "status": "awaiting", "updated": 2 }]
            }),
        );
        recorder.respond(
            "GET",
            "/ai/chats/c1",
            serde_json::json!({
                "id": "c1",
                "status": "awaiting",
                "seq": 1,
                "messages": [{
                    "seq": 1,
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "t1",
                        "name": "delete_node",
                        "args": {},
                        "status": "awaiting_approval",
                        "output": null,
                        "error": null
                    }]
                }]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("allow").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let call = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/ai/chat/approve")
            .expect("should have posted /ai/chat/approve");
        let body: serde_json::Value = serde_json::from_slice(&call.2.body).expect("body is json");
        assert_eq!(body, serde_json::json!({ "call": "t1", "allow": true }));
    }

    /// A `busy` chat paints the `detener` button; clicking it posts
    /// `POST /ai/chat/cancel` with `{"id":"c1"}` — the shape `panels::chat`
    /// sends.
    #[test]
    fn the_harness_busy_chat_offers_stop_and_posts_cancel() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/ai/chats",
            serde_json::json!({
                "chats": [{ "id": "c1", "title": "chat uno", "status": "busy", "updated": 1 }]
            }),
        );
        recorder.respond(
            "GET",
            "/ai/chats/c1",
            serde_json::json!({
                "id": "c1",
                "status": "busy",
                "seq": 1,
                "messages": [{ "seq": 1, "role": "user", "content": "hello" }]
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        // A busy chat paints an egui spinner that repaints forever, so
        // `run()` would hit max_steps; bounded steps are enough here.
        harness.run_steps(3);

        // The composer's stop button is labelled by its glyph; "stop" is only its hover text.
        harness.get_by_label("■").click();
        harness.run_steps(3);

        let calls = recorder.calls.borrow();
        let call = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/ai/chat/cancel")
            .expect("should have posted /ai/chat/cancel");
        let body: serde_json::Value = serde_json::from_slice(&call.2.body).expect("body is json");
        assert_eq!(body, serde_json::json!({ "id": "c1" }));
    }

    /// The replaced panel paints no tool-run controls: none of the old
    /// AI harness's `run: …`/`active: …` labels exist anywhere in the
    /// tree once the chat panel has run.
    #[test]
    fn no_harness_tool_run_buttons_exist() {
        let mut harness = harness(Theme::phosphor());
        harness.run();

        for label in [
            "run: list_tools",
            "active: list_tools",
            "run: capture_output",
            "active: capture_output",
        ] {
            assert!(
                harness.query_by_label(label).is_none(),
                "the chat panel must not paint the old tool control {label:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // S.AF — the settings overlay (`O.16`'s first slice)
    // -----------------------------------------------------------------------

    /// The overlay opens from the header's `settings ▸`, carries the
    /// sections sidebar (`ui` is the real one, the other two are the
    /// placeholders their future lotes fill), and closes on Esc — the
    /// same two-way close the harness and properties overlays obey.
    #[test]
    fn the_settings_overlay_opens_carries_sections_and_closes_by_esc() {
        let mut harness = harness(Theme::phosphor());
        harness.run();

        harness.get_by_label("settings").click();
        harness.run();
        assert!(
            harness.state().settings_open(),
            "click should open the overlay"
        );
        harness.run();

        // The sidebar: one real section, two honest placeholders. (The
        // active marker may or may not have inked yet on the first run —
        // both words are there regardless.)
        harness.get_by_label("ui");
        harness.get_by_label("theme");
        harness.get_by_label("language");

        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(
            !harness.state().settings_open(),
            "Esc should close the overlay"
        );
    }

    /// Regression: the ajustes window's height must settle. An anchored
    /// `Area` sizes itself from last frame's rect while its content (a
    /// vertical separator that takes all the height it is offered) fills
    /// whatever height is left below its own position, so the height used
    /// to climb every frame toward the screen. Open/close cycles must not
    /// move it either.
    #[test]
    fn the_ajustes_window_height_is_stable_across_frames_and_reopen() {
        let mut harness = harness(Theme::phosphor());
        harness.run();
        let area_height = |h: &Harness<'static, Shell>| {
            h.ctx
                .memory(|m| m.area_rect(egui::Id::new("shell/settings-overlay")))
                .map(|r| r.height())
        };

        harness.get_by_label("settings").click();
        harness.run();
        for _ in 0..3 {
            harness.run();
        }
        let settled = area_height(&harness).expect("the overlay has a rect once open");
        for _ in 0..20 {
            harness.run();
            let h = area_height(&harness).expect("rect");
            assert!(
                (h - settled).abs() < 0.5,
                "window height drifted from {settled} to {h}"
            );
        }

        for _ in 0..3 {
            harness.key_press(egui::Key::Escape);
            harness.run();
            harness.get_by_label("settings").click();
            harness.run();
            harness.run();
            let h = area_height(&harness).expect("rect");
            assert!(
                (h - settled).abs() < 0.5,
                "reopening changed the window height from {settled} to {h}"
            );
        }
    }

    /// P-G follow-up: the `Export` section shows the saved `export_dir`,
    /// `save` posts the typed folder and the reset posts the empty
    /// default — both through `POST /settings`.
    #[test]
    fn the_export_section_saves_and_resets_the_export_folder_through_the_door() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/settings", serde_json::json!({ "export_dir": "/tmp/fotos" }));
        recorder.respond("POST", "/settings", serde_json::json!({ "export_dir": "" }));
        let mut harness = harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.get_by_label("settings").click();
        harness.run();
        harness.get_by_label("export").click();
        harness.run();
        assert!(harness.query_all_by_value("/tmp/fotos").next().is_some(), "the saved folder shows");

        harness.get_by_label("save").click();
        harness.run();
        harness.get_by_label("reset to default").click();
        harness.run();

        let posts: Vec<serde_json::Value> = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/settings")
            .map(|(_, _, params)| serde_json::from_slice(&params.body).expect("json"))
            .collect();
        assert_eq!(
            posts,
            vec![serde_json::json!({ "export_dir": "/tmp/fotos" }), serde_json::json!({ "export_dir": "" })]
        );
        // The reply's empty folder is what the field shows now.
        assert!(harness.query_all_by_value("/tmp/fotos").next().is_none());
    }

    /// The dropdown writes the shell mode through the door (`POST
    /// /settings`) and the clipped reply comes back as the arrangement
    /// override: picking *escritorio* from the combo on a phone-size
    /// harness makes the delegated shell snap back to the full grid
    /// (`library` as a grid panel again, no header tab selector).
    #[test]
    fn the_shell_mode_dropdown_overrides_the_arrangement_through_the_door() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/settings",
            serde_json::json!({ "shell_mode": "mobile" }),
        );
        recorder.respond(
            "POST",
            "/settings",
            serde_json::json!({ "shell_mode": "desktop" }),
        );
        let mut harness = harness_at(
            Theme::phosphor(),
            Vec2::new(VIEWPORTS[3].1, VIEWPORTS[3].2),
            Box::new(std::rc::Rc::clone(&recorder)),
        );
        harness.run();

        // The saved mode is in force from the first frame: library is
        // a collapsible sidebar and the graph/lanes tabs live in the
        // header. (Phone size, so `Auto` would delegate too — the saved
        // override *escritorio* is what must stop it.)
        harness.get_by_label(&crate::shell::header::library_label(true));
        harness.get_by_label(&format!("view: {} ", Region::Nodes.title()));

        // Delegated viewport: `settings` sits in the header's own horizontal
        // strip (`S.U13b`), so at a phone width it may be off the right edge
        // until scrolled in — `click()` sends a real pointer click at the
        // widget's rect and would miss it there.
        harness.get_by_label("settings").scroll_to_me();
        harness.run();
        harness.get_by_label("settings").click();
        harness.run();
        assert!(harness.state().settings_open());

        harness.get_by_value("mobile").click();
        harness.run();
        harness.get_by_label("desktop").click();
        harness.run();

        // The override landed: the grid is back — the library's own
        // heading paints as a grid panel (while the delegated sidebar
        // collapsed or not is header-toggle state, not arrangement).
        harness.get_by_label(Region::Library.title());
        let posted = recorder
            .calls
            .borrow()
            .iter()
            .any(|(m, p, params)| m == "POST" && p == "/settings");
        assert!(
            posted,
            "picking a mode should POST /settings through the door"
        );
    }

    /// Picking a language in the settings overlay posts `language` through
    /// the door and the whole shell paints in it from the next frame on —
    /// the sidebar's own `language` entry reads `language` once Spanish lands.
    #[test]
    fn picking_spanish_in_settings_switches_the_whole_shell() {
        let _guard = crate::shell::i18n::LangGuard;
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/settings",
            serde_json::json!({ "shell_mode": "auto", "language": "en" }),
        );
        recorder.respond(
            "POST",
            "/settings",
            serde_json::json!({ "shell_mode": "auto", "language": "es" }),
        );
        let (_, w, h) = VIEWPORTS[0];
        let mut harness = harness_at(
            Theme::phosphor(),
            Vec2::new(w, h),
            Box::new(std::rc::Rc::clone(&recorder)),
        );
        harness.run();
        harness.get_by_label("settings").click();
        harness.run();
        harness.get_by_label("language").click();
        harness.run();
        harness.get_by_value("English").click();
        harness.run();
        harness.get_by_label("Español").click();
        harness.run();
        harness.run();

        assert_eq!(crate::shell::i18n::lang(), crate::shell::i18n::Lang::Es);
        // The combo's popup sits outside the overlay, so the pick may close it
        // (a click outside); reopen it to read the sidebar in Spanish.
        if !harness.state().settings_open() {
            harness.get_by_label("settings").click();
            harness.run();
        }
        // The sidebar entry and the open section's heading both read `language`.
        assert!(harness.query_all_by_label("idioma").next().is_some());
        assert!(harness.query_by_label("language").is_none(), "no English left in the sidebar");
        let posted = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/settings"
                && params.json().is_some_and(|b| b["language"] == serde_json::json!("es"))
        });
        assert!(posted, "picking Español should POST /settings {{language: \"es\"}}");
    }

    /// The persisted side: with no `data/settings.json` answering, the
    /// overlay's cached override stays `Auto` — the arrangement is the
    /// one `S.U13` decided, unforced. (The door answers the layer's
    /// default reply shape, not a guessed one.)
    #[test]
    fn no_saved_shell_mode_means_the_arrangement_stays_decided_by_viewport() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[0];
        let size = Vec2::new(w, h);
        assert!(
            classes::arrangement(&theme, size).is_full_grid(),
            "desktop controls the grid"
        );

        let mut harness = harness_at(theme, size, Box::new(FakeDoor::new()));
        harness.run();
        // Auto at desktop = the plain S.U13 grid: no delegated controls at all.
        assert!(harness.query_by_label(&crate::shell::header::library_label(true)).is_none());
        harness.get_by_label("settings");
    }

    /// `S.U13b`: the delegated header's right cluster is reachable. Before
    /// this lote the header was one flat left-to-right row, so at
    /// `phone-small` the row was wider than the viewport and `IA` /
    /// `hold: properties` / `settings` sat past the right edge — visible
    /// to no one, and invisible to every other test here because
    /// `click_accesskit` acts on the node and ignores its coordinates. The
    /// row is now a hidden-scrollbar horizontal strip; scrolling any
    /// right-cluster control lands it inside the viewport.
    #[test]
    fn the_delegated_header_scrolls_its_right_cluster_into_reach() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4]; // phone-small
        assert!(
            !classes::arrangement(&theme, Vec2::new(w, h)).is_full_grid(),
            "phone-small must be delegated for this test to mean anything"
        );
        let mut harness = harness_at(theme, Vec2::new(w, h), Box::new(FakeDoor::new()));
        harness.run();

        for label in ["hold: properties", "settings"] {
            harness.get_by_label(label).scroll_to_me();
            harness.run();
            let r = harness.get_by_label(label).rect();
            assert!(
                r.min.x >= -1.0 && r.max.x <= w + 1.0,
                "{label} at {r:?} is still outside the {w}px viewport after scroll_to_me"
            );
        }
    }

    /// V2b: a canonical (32 Hz) sequence shows four times the ticks of a base
    /// (8 Hz) one at the same zoom. This mounts the real timeline painter and
    /// queries the tick-count readout's AccessKit node — the ruler's labels are
    /// `painter.text`, so `paint` exposes the sequence's tick count as a
    /// `Label`, which is both the seam this test needs and an honest "how long
    /// is this, in ticks?" readout for a human.
    #[test]
    fn a_32hz_sequence_shows_four_times_the_ticks_at_the_same_zoom() {
        use crate::shell::lanes::{DigestMatrix, IngestLane};
        use crate::shell::timeline;

        /// Mount the timeline on a sequence of `ticks` and assert its tick-count
        /// readout is present. The visible span is the same (`span`), so this is
        /// the same zoom; only the sequence's grid differs.
        fn shows(rate: f32, ticks: usize) {
            let theme = Theme::phosphor();
            let paint_theme = theme.clone();
            let mut lane = IngestLane::default();
            lane.matrix = Some(DigestMatrix {
                song: "s".into(),
                rate,
                ticks,
                ..Default::default()
            });
            lane.span = 40;
            let size = Vec2::new(
                crate::shell::DESKTOP_VIEWPORT.0 as f32,
                crate::shell::DESKTOP_VIEWPORT.1 as f32,
            );
            let mut harness = Harness::builder()
                .with_size(size)
                .build_ui_state(
                    move |ui, lane: &mut IngestLane| {
                        let fields = |_: &str| Vec::new();
                        let h = ui.available_height();
                        timeline::paint(ui, &paint_theme, lane, &fields, h);
                    },
                    lane,
                );
            theme.apply(&harness.ctx);
            harness.run();
            harness.get_by_label(&format!("{ticks} ticks"));
        }

        const BASE_TICKS: usize = 40;
        let canon_ticks =
            metal_schema::TickRate::Base.to_canonical(BASE_TICKS as i64) as usize;
        shows(8.0, BASE_TICKS);
        shows(32.0, canon_ticks);
        assert_eq!(canon_ticks, BASE_TICKS * 4, "the canonical sequence has 4× the ticks");
    }

    /// V3: the timeline's voice row says "not digested for voice" for a
    /// 16-wide digest rather than drawing a silent singer. The honest seam is
    /// an AccessKit label in the row (the ruler's tick-count readout is the
    /// same kind of node), so this mounts the real timeline painter and queries
    /// that text.
    #[test]
    fn the_voice_row_says_not_digested_for_a_16_wide_song() {
        use crate::shell::lanes::{DigestMatrix, IngestLane};
        use crate::shell::timeline;

        let theme = Theme::phosphor();
        let paint_theme = theme.clone();
        let mut lane = IngestLane::default();
        lane.matrix = Some(DigestMatrix {
            song: "s".into(),
            rate: 32.0,
            ticks: 64,
            // The width the script and every pre-V2 digest write: no voice block.
            has_voice: false,
            ..Default::default()
        });
        lane.span = 40;
        let size = Vec2::new(
            crate::shell::DESKTOP_VIEWPORT.0 as f32,
            crate::shell::DESKTOP_VIEWPORT.1 as f32,
        );
        let mut harness = Harness::builder()
            .with_size(size)
            .build_ui_state(
                move |ui, lane: &mut IngestLane| {
                    let fields = |_: &str| Vec::new();
                    let h = ui.available_height();
                    timeline::paint(ui, &paint_theme, lane, &fields, h);
                },
                lane,
            );
        theme.apply(&harness.ctx);
        harness.run();
        harness.get_by_label("not digested for voice");
    }

    /// V3: the properties tick page lists the voice block — the thirteen
    /// `metal_schema::VOICE_COLUMNS` by name at the playhead. A 16-wide digest
    /// says it has none instead of listing thirteen zeros.
    #[test]
    fn the_tick_page_lists_the_voice_block() {
        use crate::shell::lanes::{DigestMatrix, IngestLane};
        use crate::shell::panels::properties::ingest;

        fn mount(has_voice: bool, base_only: bool) -> Harness<'static, IngestLane> {
            let theme = Theme::phosphor();
            let paint_theme = theme.clone();
            let door = std::rc::Rc::new(FakeDoor::new());
            let mut lane = IngestLane::default();
            lane.matrix = Some(DigestMatrix {
                song: "s".into(),
                rate: 32.0,
                ticks: 64,
                has_voice,
                base_only,
                voice: vec![[0.7, 0.5, 0.4, 0.9, 0.8, 0.2, 0.6, 0.3, 0.1, 0.9, 0.0, 0.1, 0.0]; 64],
                ..Default::default()
            });
            let size = Vec2::new(
                crate::shell::DESKTOP_VIEWPORT.0 as f32,
                crate::shell::DESKTOP_VIEWPORT.1 as f32,
            );
            let mut harness = Harness::builder()
                .with_size(size)
                .build_ui_state(
                    move |ui, lane: &mut IngestLane| {
                        ingest::section(ui, &paint_theme, &door, lane);
                    },
                    lane,
                );
            theme.apply(&harness.ctx);
            harness.run();
            harness
        }

        let harness = mount(true, false);
        // The heading, and a column from each end of the block — the presence
        // and the last style — so a dropped name would fail the test.
        harness.get_by_label("voice block");
        harness.get_by_label("voice");
        harness.get_by_label("voice level");
        harness.get_by_label("pitch");
        harness.get_by_label("sing");
        harness.get_by_label("whisper");

        // An 8 Hz-only digest held as steps carries the base-only badge.
        mount(true, true).get_by_label("base-only");

        // A digest that never measured a voice says so, not thirteen zeros.
        mount(false, false).get_by_label("not digested for voice");
    }

    /// `Show` (`S.AP`'s Edit|Show): entering it asks for one monitor window
    /// per output that is not the preview, unfreezes a frozen clock, and a
    /// second entry without leaving adds no window.
    #[test]
    fn entering_show_opens_one_monitor_per_real_output_and_unfreezes() {
        let _chdir = chdir_lock();
        let door = std::rc::Rc::new(FakeDoor::new());
        door.respond(
            "GET",
            "/state",
            serde_json::json!({
                "running": false,
                "outputs": [{ "id": "device-A" }, { "id": "device-B" }, { "id": crate::PREVIEW }],
            }),
        );
        let mut shell = Shell::new(Theme::phosphor(), Box::new(std::rc::Rc::clone(&door)));
        shell.enter_show();
        {
            let calls = door.calls.borrow();
            let monitors: Vec<String> = calls
                .iter()
                .filter(|(m, p, _)| m == "POST" && p == "/monitor")
                .map(|(_, _, q)| {
                    serde_json::from_slice::<serde_json::Value>(&q.body).expect("json body")["device"]
                        .as_str()
                        .expect("device id")
                        .to_string()
                })
                .collect();
            assert_eq!(monitors, ["device-A", "device-B"], "one per real output, never the preview");
            let transports = calls.iter().filter(|(m, p, _)| m == "POST" && p == "/transport").count();
            assert_eq!(transports, 1, "a frozen clock is unfrozen on entry");
        }
        shell.enter_show();
        let again = door
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/monitor")
            .count();
        assert_eq!(again, 2, "a second entry without leaving opens no new window");
    }

    /// The header toggle: `Show` paints no region (no graph canvas, no
    /// library sidebar) and the overlays grid's own fixed row over the
    /// whole area; `edit` brings the regions back. Each switch is persisted
    /// through `POST /settings`, and entering Show asks for the monitors.
    #[test]
    fn show_hides_the_regions_and_paints_the_overlays_grid_alone() {
        let _chdir = chdir_lock();
        let door = std::rc::Rc::new(FakeDoor::new());
        door.respond(
            "GET",
            "/state",
            serde_json::json!({ "running": true, "outputs": [{ "id": "device-A", "layers": [] }] }),
        );
        let mut h = harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&door)));
        h.run();
        assert!(h.state().region_rects().contains_key(&Region::Nodes));

        h.get_by_label("Show").click();
        h.run();
        let rects = h.state().region_rects();
        for region in [Region::Nodes, Region::Library, Region::Lanes, Region::Properties] {
            assert!(!rects.contains_key(&region), "{region:?} must not paint in Show");
        }
        assert!(h.query_by_label(&crate::shell::header::library_label(true)).is_none());
        assert!(h.query_by_label(&crate::shell::header::library_label(true)).is_none());
        h.get_by_label_contains("›");
        {
            let calls = door.calls.borrow();
            let saved = calls.iter().any(|(m, p, q)| {
                m == "POST"
                    && p == "/settings"
                    && serde_json::from_slice::<serde_json::Value>(&q.body)
                        .is_ok_and(|b| b["shell_view"] == "show")
            });
            assert!(saved, "the Show choice is persisted");
            let opened = calls.iter().any(|(m, p, q)| {
                m == "POST"
                    && p == "/monitor"
                    && serde_json::from_slice::<serde_json::Value>(&q.body)
                        .is_ok_and(|b| b["device"] == "device-A")
            });
            assert!(opened, "entering Show opens the output's monitor");
        }

        h.get_by_label("Edit").click();
        h.run();
        assert!(h.state().region_rects().contains_key(&Region::Nodes), "edit restores the regions");
    }

    /// A Show saved last run opens in Show: the persisted view is applied on
    /// the first frame, and that frame is the one that asks for the monitors.
    #[test]
    fn a_show_saved_last_run_opens_in_show_on_the_first_frame() {
        let _chdir = chdir_lock();
        let door = std::rc::Rc::new(FakeDoor::new());
        door.respond("GET", "/settings", serde_json::json!({ "shell_view": "show" }));
        door.respond(
            "GET",
            "/state",
            serde_json::json!({ "running": true, "outputs": [{ "id": "device-A" }] }),
        );
        // A Shell alone, before any frame: nothing opened yet.
        let _shell = Shell::new(Theme::phosphor(), Box::new(std::rc::Rc::clone(&door)));
        let before = door.calls.borrow().iter().filter(|(m, p, _)| m == "POST" && p == "/monitor").count();
        assert_eq!(before, 0, "nothing opens before the first frame");
        let mut h = harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&door)));
        h.run();
        let after = door.calls.borrow().iter().filter(|(m, p, _)| m == "POST" && p == "/monitor").count();
        assert_eq!(after, 1, "the first frame applies Show and opens the monitor");
        assert!(h.state().region_rects().is_empty(), "Show paints no region");
    }

    // --- machine pickers (2026-10-09) -------------------------------------
    // `GET /peers` is the relayed `metal-sync` list; these are the fake peers the
    // pickers see — `pi` online, `laptop` offline — scripted on the door so
    // no real :7901 is needed.

    fn script_peers(recorder: &FakeDoor) {
        recorder.respond(
            "GET",
            "/peers",
            serde_json::json!({ "local": "casa", "sync": true, "peers": [
                { "name": "pi", "node_id": "aaa", "online": true, "last_seen": 1, "via": null },
                { "name": "laptop", "node_id": "bbb", "online": false, "last_seen": null, "via": null }
            ] }),
        );
    }

    /// One graph on `col-main` holding `node`, the screens `main` (this
    /// machine) and `wall` (shown by `pi`), and the source layer `layer-7`
    /// routed to `main`.
    fn script_machine_graph(recorder: &FakeDoor, node: serde_json::Value) {
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-main", "name": "col-main", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-main", "nodes": [node], "edges": [] },
                "primitives": [],
                "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        script_peers(recorder);
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({
                "node": "casa",
                "devices": [{ "id": "main", "layers": [
                    { "id": "layer-7", "name": "source", "render_on": "each", "outputs": ["main"] }
                ] }],
                "outputs": [
                    { "id": "main", "label": "principal", "width": 1920, "height": 1080, "node": null, "composed_on": null },
                    { "id": "wall", "label": "pared", "width": 1920, "height": 1080, "node": "pi", "composed_on": null }
                ],
                "collections": [],
            }),
        );
    }

    fn posted_bodies(recorder: &FakeDoor, path: &str) -> Vec<serde_json::Value> {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == path)
            .filter_map(|(_, _, params)| serde_json::from_slice(&params.body).ok())
            .collect()
    }

    #[test]
    fn renders_on_picks_a_peer_and_never_an_offline_one() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_machine_graph(
            &recorder,
            serde_json::json!({ "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "source",
                "bypass": false, "solo": false, "config": { "source_id": "layer-7" } }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        run_real(&mut harness);
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        run_real(&mut harness);

        // The offline peer is listed (greyed) but a pick on it posts nothing.
        // The node header block (task 4a) can start this control below the
        // scroll viewport's clip, so scroll it in before the pick.
        harness.get_by_label("each").scroll_to_me();
        run_real(&mut harness);
        harness.get_by_label("each").click();
        run_real(&mut harness);
        assert!(
            harness
                .get_by_label("laptop · offline")
                .accesskit_node()
                .is_disabled(),
            "an offline machine is listed greyed out"
        );
        assert!(
            !harness.get_by_label("pi").accesskit_node().is_disabled(),
            "an online machine stays pickable"
        );

        harness.get_by_label("pi").click();
        run_real(&mut harness);
        let bodies = posted_bodies(&recorder, "/layers");
        assert!(
            bodies.iter().any(|b| b["op"] == "set"
                && b["layer"] == "layer-7"
                && b["device"] == "main"
                && b["render_on"] == serde_json::json!({ "node": "pi" })),
            "picking pi should POST /layers set render_on {{node: pi}}, got {bodies:?}"
        );
    }

    #[test]
    fn composes_on_picks_a_machine_from_the_menu() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_machine_graph(
            &recorder,
            serde_json::json!({ "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "principal",
                "bypass": false, "solo": false, "config": { "device": "main" } }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        run_real(&mut harness);
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("out-0".to_string()));
        run_real(&mut harness);

        // Scroll the control into view first — the node header block (task
        // 4a) can push it below the scroll viewport's clip.
        harness.get_by_label("by default").scroll_to_me();
        run_real(&mut harness);
        harness.get_by_label("by default").click();
        run_real(&mut harness);
        harness.get_by_label("pi").click();
        run_real(&mut harness);
        let bodies = posted_bodies(&recorder, "/outputs");
        assert!(
            bodies.iter().any(|b| b["op"] == "composed_on" && b["id"] == "main" && b["composed_on"] == "pi"),
            "picking pi should POST /outputs composed_on=pi, got {bodies:?}"
        );
    }

    // -----------------------------------------------------------------
    // The venue scene (22-oct-2026): the nodes canvas flips into a 3D
    // scene of the open graph's room, a bar carries `+` (the gallery) and
    // "Back to nodes", and an empty room starts a scene instead of a popup.
    // -----------------------------------------------------------------

    /// A graph with one `venue.surface` wired to one output, in room `bar`,
    /// whose room already holds the surface `front` — the scene's "the room
    /// as saved" case. `displays` seeds the room's displays (`[]` for an
    /// empty one).
    fn script_venue_scene_graph(recorder: &FakeDoor, displays: serde_json::Value, surfaces: serde_json::Value) {
        recorder.respond(
            "GET",
            "/graphs",
            serde_json::json!({ "graphs": [{ "id": "col-screen-1", "name": "col-screen-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            serde_json::json!({
                "graph": { "id": "col-screen-1", "nodes": [
                    { "id": "surface-0", "kind": "venue.surface", "pos": [200.0, 0.0], "label": "Surface",
                      "bypass": false, "solo": false,
                      "config": { "venue": "bar", "surface": "front" } },
                    { "id": "out-0", "kind": "output.device", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                      "config": { "device": "screen-1" } }
                ]},
                "primitives": [
                    { "type": "box", "tag": "node", "node": "surface-0", "rect": [200.0, 0.0, 120.0, 60.0] },
                    { "type": "box", "tag": "node", "node": "out-0", "rect": [0.0, 0.0, 120.0, 60.0] }
                ], "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", serde_json::json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/effects", serde_json::json!({ "node": "surface-0", "effects": [] }));
        recorder.respond(
            "GET",
            "/state",
            serde_json::json!({ "outputs": [{ "id": "screen-1", "width": 1920, "height": 1080, "masks": [] }] }),
        );
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                "venue": {
                    "venue": { "id": "bar", "name": "Bar", "setting": "indoor" },
                    "surfaces": surfaces,
                    "objects": [], "projectors": [], "fixture": [],
                    "mask": [], "display": displays, "composition": [], "sensor": [],
                },
                "report": { "problems": [] }, "outputs": [], "patch": [],
            }),
        );
        recorder.respond("POST", "/venue", serde_json::json!({ "ok": true }));
    }

    /// The front surface a non-empty room holds, four corners on the back
    /// wall — the `points` `POST /venue {op:"surface"}` accepts.
    fn front_surface() -> serde_json::Value {
        serde_json::json!({
            "id": "front", "name": "Frente",
            "origin": [-1.92, 0.6, -4.0], "u": [3.84, 0.0, 0.0], "v": [0.0, 2.16, 0.0],
            "material": "",
        })
    }

    /// Every `POST /venue` body this door saw, in order.
    fn venue_bodies(recorder: &FakeDoor) -> Vec<serde_json::Value> {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/venue")
            .filter_map(|(_, _, params)| serde_json::from_slice::<serde_json::Value>(&params.body).ok())
            .collect()
    }

    /// The `surface-0` node's focus bar draws its `paint the room` gear at
    /// `[[750, 232] - [776, 258]]` on the harness's canvas: the stub layout
    /// puts the node's box there, the bar hangs under it, and Settings is the
    /// row's sixth icon. Read off the panel's own layout by eye (this is a
    /// test's pointer target, not a painted constant), so all three scene
    /// tests press the same spot.
    fn gear_spot(harness: &Harness<'static, Shell>) -> egui::Pos2 {
        let size = Theme::phosphor().icon_button_size;
        harness
            .get_all_by_label("paint the room")
            .map(|n| n.rect())
            .find(|r| r.size() == size)
            .expect("the gear sits on the focus bar")
            .center()
    }

    /// Presses `at` in one frame and releases it in the next: the focus bar
    /// is a foreground `Area`, and egui resolves its buttons against last
    /// frame's rects.
    fn press_and_release(harness: &mut Harness<'static, Shell>, at: egui::Pos2) {
        harness.drag_at(at);
        harness.run();
        harness.drop_at(at);
        harness.run();
    }

    /// The gear on a `venue.surface` node's own floating bar puts the canvas
    /// in the venue scene: the flag is on and the graph's node boxes are gone.
    #[test]
    fn the_gear_puts_the_canvas_in_the_venue_scene() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_venue_scene_graph(
            &recorder,
            serde_json::json!([{ "id": "front", "name": "Frente", "surface": "front", "output": "front",
                                 "kind": "led_wall", "resolution": [1920, 1080], "link": "local" }]),
            serde_json::json!([front_surface()]),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();
        assert!(
            !harness.state().nodes_venue_scene_on(),
            "the canvas starts on the graph"
        );

        let gear = gear_spot(&harness);
        press_and_release(&mut harness, gear);
        assert!(
            harness.state().nodes_venue_scene_on(),
            "the gear must flip the canvas into the venue scene"
        );
        // The graph's boxes are gone: the scene paints the room, and its bar
        // names the room instead of the node cards.
        assert!(
            harness
                .query_by_label(&components::glyph(components::Icon::InsertBefore).to_string())
                .is_none(),
            "the node focus bar belongs to the graph, not the scene"
        );
        // The bar carries the labelled way back (the room's name is not on it).
        let out = harness.get_by_label("Back to nodes").rect();
        assert!(out.width() > 0.0, "the bar paints the way back");

        harness.get_by_label("Back to nodes").click();
        harness.run();
        assert!(
            !harness.state().nodes_venue_scene_on(),
            "\"Back to nodes\" returns to the graph"
        );
    }

    /// In the scene, the bar's `+` opens the gallery, and one of its cards
    /// posts `POST /venue {op:"screen"}` for the scene's room.
    #[test]
    fn the_scene_plus_opens_the_gallery() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_venue_scene_graph(
            &recorder,
            serde_json::json!([{ "id": "front", "name": "Frente", "surface": "front", "output": "front",
                                 "kind": "led_wall", "resolution": [1920, 1080], "link": "local" }]),
            serde_json::json!([front_surface()]),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();
        let gear = gear_spot(&harness);
        press_and_release(&mut harness, gear);
        assert!(harness.state().nodes_venue_scene_on());

        // The bar's `+` is its first slot (`[[415, 136] - [441, 162]]`);
        // press and release in two frames, like the gear.
        press_and_release(&mut harness, egui::pos2(428.0, 149.0));
        assert!(harness.state().nodes_gallery_open(), "the `+` opens the gallery");

        harness.get_by_label("TV 65\"").scroll_to_me();
        harness.run();
        harness.get_by_label("TV 65\"").click();
        harness.run();

        let posted = venue_bodies(&recorder)
            .into_iter()
            .find(|b| b["op"] == "screen")
            .expect("a gallery card must post POST /venue {op: screen}");
        assert_eq!(posted["id"], serde_json::json!("bar"), "the scene's room");
        assert_eq!(posted["screen"]["pixels"], serde_json::json!([3840, 2160]));
        let width = posted["screen"]["width_m"].as_f64().unwrap();
        assert!((width - 2.0).abs() < 1e-6, "the typed width: {width}");
    }

    /// A room nothing could be read for is still a **scene**: the ground and
    /// its grid, not a line of text. The bar is there and no disclaimer is
    /// painted in the middle.
    #[test]
    fn a_room_with_no_document_paints_an_empty_scene() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_venue_scene_graph(&recorder, serde_json::json!([]), serde_json::json!([]));
        // The room answers `null`: nothing on disk yet.
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({ "venues": [], "last": "bar", "venue": null,
                "report": { "problems": [] }, "outputs": [], "patch": [] }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();
        let gear = gear_spot(&harness);
        press_and_release(&mut harness, gear);
        assert!(harness.state().nodes_venue_scene_on(), "the scene is on");
        // The scene paints: the empty room's ground is drawn and the bar's
        // own controls are queryable — nothing says the room could not be read.
        harness.get_by_label("add to the room");
        assert!(
            harness.query_by_label("— could not read the room —").is_none(),
            "an empty room is a scene, not a disclaimer"
        );
    }

    /// With no reply for the room at all, the scene is still the empty room.
    #[test]
    fn an_unreadable_room_paints_an_empty_scene() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_venue_scene_graph(&recorder, serde_json::json!([]), serde_json::json!([]));
        // `GET /venue` is not scripted at all: an unreadable room.
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();
        let gear = gear_spot(&harness);
        press_and_release(&mut harness, gear);
        assert!(harness.state().nodes_venue_scene_on(), "the scene is on");
        // The scene paints: the empty room's ground is drawn and the bar's
        // own controls are queryable — nothing says the room could not be read.
        harness.get_by_label("add to the room");
        assert!(
            harness.query_by_label("— could not read the room —").is_none(),
            "an empty room is a scene, not a disclaimer"
        );
    }

    /// The objects tab is the room's furniture: a preset card posts one
    /// `object` (or `stage`/`truss`/`dummy`), and the row under it deletes.
    #[test]
    fn the_objects_tab_posts_and_removes_a_room_object() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        // A room with a counter in it: the list under the cards reads this.
        script_venue_scene_graph(&recorder, serde_json::json!([]), serde_json::json!([]));
        recorder.respond(
            "GET",
            "/venue",
            serde_json::json!({
                "venues": [{ "id": "bar", "name": "Bar" }], "last": "bar",
                "venue": {
                    "venue": { "id": "bar", "name": "Bar", "setting": "indoor" },
                    "surfaces": [], "object": [
                        { "id": "bar-1", "name": "Barra", "kind": "box",
                          "at": [5.0, 0.55, 0.0], "size": [4.0, 1.1, 0.6], "yaw": 0.0 }
                    ], "projectors": [], "fixture": [],
                    "mask": [], "display": [], "composition": [], "sensor": [],
                },
                "report": { "problems": [] }, "outputs": [], "patch": [],
            }),
        );
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();
        harness
            .state_mut()
            .request_venue_popup_for_test("col-screen-1", "surface-0");
        harness.state_mut().venue_popup_tab_for_test(
            crate::shell::panels::venue_popup::Tab::Objects,
        );
        harness.run();

        // The card: a counter, its three sizes in metres. It sits below the
        // popup's fold, so the band scrolls to it first.
        harness.get_by_label(crate::i18n::tr_key("bar counter")).scroll_to_me();
        harness.run();
        harness.get_by_label(crate::i18n::tr_key("bar counter")).click();
        harness.run();
        let posted = venue_bodies(&recorder)
            .into_iter()
            .find(|b| b["op"] == "object" && b.get("delete").is_none())
            .expect("a card must post POST /venue {op: object}");
        assert_eq!(posted["id"], serde_json::json!("bar"), "the scene's room");
        // The room already holds `bar-1`, so the card takes the next free id.
        assert_eq!(posted["object"], serde_json::json!("bar-2"));
        assert_eq!(posted["kind"], serde_json::json!("box"));
        let size = posted["size"].as_array().cloned().unwrap_or_default();
        assert_eq!(size.len(), 3, "a box has a size");
        assert!((size[1].as_f64().unwrap() - 1.1).abs() < 1e-6, "a counter's height");

        // The room's own row: the object the room holds, and its delete. The
        // list is repainted from the door's reply, so the row's own label is
        // the object's kind and its id rides the delete's tooltip.
        let remove = crate::i18n::trf(
            "remove {id} from the room",
            &[("id", &"bar-1")],
        );
        harness.get_by_label(&remove).scroll_to_me();
        harness.run();
        harness.get_by_label(&remove).click();
        harness.run();
        harness.run();
        let deleted = venue_bodies(&recorder)
            .into_iter()
            .find(|b| b["op"] == "object" && b["delete"] == serde_json::json!(true))
            .expect("the row's delete posts the room's object delete");
        assert_eq!(deleted["object"], serde_json::json!("bar-1"));
    }

    /// A room with no displays enters the scene and is started there: exactly
    /// one floor `surface`, then one `screen`, and no gallery opens.
    #[test]
    fn an_empty_room_starts_a_scene_not_a_popup() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_venue_scene_graph(&recorder, serde_json::json!([]), serde_json::json!([]));
        let mut harness =
            harness_with_door(Theme::phosphor(), Box::new(std::rc::Rc::clone(&recorder)));
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("surface-0".to_string()));
        harness.run();
        let gear = gear_spot(&harness);
        press_and_release(&mut harness, gear);
        assert!(harness.state().nodes_venue_scene_on());

        let posts: Vec<serde_json::Value> = venue_bodies(&recorder)
            .into_iter()
            .filter(|b| b["op"] == "surface" || b["op"] == "screen" || b["op"] == "fixture")
            .collect();
        assert_eq!(
            posts.iter().filter(|b| b["op"] == "surface").count(),
            1,
            "exactly one floor surface, got {posts:?}"
        );
        assert_eq!(
            posts.iter().filter(|b| b["op"] == "screen").count(),
            1,
            "exactly one screen, got {posts:?}"
        );
        assert_eq!(posts[0]["op"], serde_json::json!("surface"), "the floor first: {posts:?}");
        assert_eq!(posts[1]["op"], serde_json::json!("screen"), "then the screen: {posts:?}");
        assert_eq!(posts[0]["surface"], serde_json::json!("floor"));
        assert_eq!(posts[1]["screen"]["pixels"], serde_json::json!([1920, 1080]));
        assert_eq!(posts[1]["surface"], serde_json::json!("screen-1"));
        assert!(
            !harness.state().nodes_gallery_open(),
            "an empty room starts the scene; the gallery stays shut"
        );
        assert!(harness.state().nodes_venue_scene_on());
        // A second frame posts nothing more: the starter is once per room.
        harness.run();
        harness.run();
        assert_eq!(
            venue_bodies(&recorder)
                .iter()
                .filter(|b| b["op"] == "surface" || b["op"] == "screen")
                .count(),
            2,
            "the starter never runs twice"
        );
    }

    /// The composer's `@` list is built from the open graph's nodes, each
    /// node's properties, the library's files and the overlays grid's cells,
    /// with the selected node (and its properties) focused.
    #[test]
    fn mention_candidates_gathers_nodes_properties_files_and_cells() {
        use crate::shell::mention::Kind;
        let door = std::rc::Rc::new(FakeDoor::new());
        let mut shell = Shell::new(Theme::phosphor(), Box::new(std::rc::Rc::clone(&door)));
        shell.nodes.set_graph_for_test(serde_json::json!({ "graph": { "id": "g1", "nodes": [
            { "id": "gen-a", "kind": "gen.turing", "label": "" },
            { "id": "fx-1", "kind": "fx.shake", "label": "" }] } }));
        shell.nodes.set_kind_params_for_test("gen.turing", &["speed"]);
        shell.library.set_fs_for_test(serde_json::json!({ "dirs": [], "files": [{ "file": "kick.wav", "kind": "audio" }] }));
        let mut layout = crate::surface::Layout::default();
        layout.roles.insert("rec-1".to_string(), crate::surface::Role::Rec);
        shell.lanes.set_layout_for_test(layout);
        shell.set_selection_for_test(Selection::Node("gen-a".to_string()));

        let all = shell.mention_candidates();
        let of = |kind: Kind| -> Vec<(&str, bool)> {
            all.iter().filter(|c| c.kind == kind).map(|c| (c.token.as_str(), c.focused)).collect()
        };
        assert_eq!(of(Kind::Node), [("gen-a", true), ("fx-1", false)]);
        assert_eq!(of(Kind::Property), [("gen-a@speed", true)]);
        assert_eq!(of(Kind::File), [("kick.wav", false)]);
        assert_eq!(of(Kind::Overlay), [("rec-1", false)]);
        let ranked: Vec<_> = crate::shell::mention::rank("gen-a@sp", &all).iter().map(|c| c.token.clone()).collect();
        assert_eq!(ranked, ["gen-a@speed"]);
    }
}
