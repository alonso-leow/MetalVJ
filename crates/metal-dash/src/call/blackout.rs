//! `S.O.1`: `GET`/`POST /overlay/blackout` — the room's off switch.
//!
//! `plan-opus.md § O.28` point 5 fixes the exact form: blackout is the
//! interruptor cero — `bypass` of every `output.device` **plus** an
//! explicit black on every `out.*` fixture, because a bypassed fixture is
//! not necessarily a *black* one (`bypass` only tells `compile`/`compose`
//! to skip rendering a node; it says nothing about the last DMX/sACN
//! values already latched on real hardware). `POST /overlay/blackout {
//! "on": bool }` writes both halves in one pass; `GET /overlay/blackout`
//! reads whether it is on fresh off the graphs (regla 4), never a stored
//! bit.
//!
//! # Why this route has its own small body, not a literal call into `switch_post`
//!
//! `metal_perform::switch::Switch` is scoped to **one** graph (`Switch.graph:
//! String`, one `set`/`unset` batch against it) — the shape `S.AK`/`O.28`
//! built for a mood or a strobo, both of which live on a single device's
//! canvas. Blackout has to reach every `output.device` **and** `out.*`
//! node in the whole show, which today is one graph per device
//! (`metal_perform::engine::device_graph_id` — confirmed against
//! `ensure_device_graph_at_puts_the_screen_in_the_one_collection`). Generalising
//! `Switch`/`SwitchEntry` to carry a graph per entry so one persisted
//! switch could span every device is real future work — worth doing the
//! day something else also needs a multi-graph switch — not something this
//! one fixed button should force through speculatively (regla 6's own
//! spirit: build the mechanism the ask needs, not the general case nobody
//! asked for yet).
//!
//! What this route *does* share, so there is still one mechanism and not
//! two competing ones: [`metal_perform::switch::apply`] itself, called once
//! per graph against a `SwitchEntry` batch built fresh from that graph's own
//! `output.device` nodes, plus [`metal_perform::fixture::FixtureConfig`]
//! (`S.O.1`'s new mechanism) for the fixture half. A future multi-graph
//! `Switch` would still call these same two primitives — it would not be a
//! rewrite, only a reshaping of what feeds them.
//!
//! **The blackout switch is never persisted to `data/switches.json`.**
//! Which nodes even exist (a device added or removed, a fixture wired in
//! mid-set) can change between two blackouts — regla 4 one level up from
//! "is a switch on": recomputing the node lists fresh on every call is what
//! keeps blackout from going stale the way a saved `set` naming yesterday's
//! devices would.

use serde_json::{json, Value};

use metal_graph::Graph;
use metal_perform::output;
use metal_perform::switch::{apply as switch_apply, SwitchEntry};

use super::{CallError, Ctx, Params};

/// Every graph file under `root/graphs/`, parsed. The same directory
/// `crate::list_graphs` (C4, read-only) walks, but this caller needs the
/// full [`Graph`] to mutate and save back rather than the summary row that
/// route answers with. A file that fails to parse is skipped — one corrupt
/// graph must not stop blackout from reaching every other output, the same
/// tolerance `list_graphs` already has.
fn every_graph_at(root: &std::path::Path) -> Vec<Graph> {
    let dir = metal_perform::engine::graphs_dir(root);
    let Ok(entries) = metal_vfs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| metal_vfs::read_to_string(e.path()).ok())
        .filter_map(|text| Graph::from_json(&text).ok())
        .collect()
}

/// Whether blackout is on right now: every `output.device` node, across
/// every graph, bypassed. An empty set (no device anywhere) is never on —
/// the same "nothing to be on *of*" rule [`metal_perform::switch::Switch::state`]
/// applies to an empty `set`.
fn is_blacked_out_at(root: &std::path::Path) -> bool {
    let graphs = every_graph_at(root);
    let mut any = false;
    for g in &graphs {
        for n in &g.nodes {
            if n.kind == "output.device" {
                any = true;
                if !n.bypass {
                    return false;
                }
            }
        }
    }
    any
}

/// Writes `on` to every graph with an `output.device` or `out.*` node:
/// `bypass` for the screens, [`metal_perform::fixture::FixtureConfig::forced`] black for the
/// fixtures. One save per graph actually touched — never one per node,
/// the same "one save, not one per node" discipline `metal_perform::switch::apply`'s
/// own module doc states, generalised here to "one save per graph" because
/// blackout spans more than one.
fn apply_at(root: &std::path::Path, on: bool) -> Result<(), String> {
    for mut g in every_graph_at(root) {
        let mut touched = false;

        // The screens: the exact `SwitchEntry`/`apply` primitive every
        // saved switch already uses — `on: true` in `SwitchEntry`'s own
        // sense means "not bypassed", the inverse of blackout's `on`
        // ("blacked out" = bypassed), hence `!on` here.
        let entries: Vec<SwitchEntry> = g
            .nodes
            .iter()
            .filter(|n| n.kind == "output.device")
            .map(|n| SwitchEntry::node(n.id.clone(), !on))
            .collect();
        if !entries.is_empty() {
            switch_apply(&mut g, &entries);
            touched = true;
        }

        // The fixtures: the half `bypass` cannot reach. `set_forced` writes
        // only `forced`, so a node's universe, unicast, heads or serial port
        // survive the blackout and its release.
        for node in g.nodes.iter_mut() {
            if node.kind.starts_with("out.") {
                metal_perform::fixture::set_forced(node, on.then_some([0.0, 0.0, 0.0]));
                touched = true;
            }
        }

        if touched {
            metal_perform::engine::save_graph_at(root, &g)?;
        }
    }
    Ok(())
}

/// `GET /overlay/blackout`.
pub fn blackout_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(json!({ "on": is_blacked_out_at(&output::data_root()) }))
}

/// `POST /overlay/blackout { "on": bool }` — the shortcut `S.O.1` asks for:
/// exactly the batch a `POST /switch {op: on|off, id: "blackout"}` would
/// run, computed fresh (see the module doc for why this is its own small
/// body rather than a literal call into `switch::switch_post`).
pub fn blackout_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let on = body["on"]
        .as_bool()
        .ok_or_else(|| CallError::Bad("expected `on` (bool)".into()))?;
    apply_at(&output::data_root(), on).map_err(CallError::Internal)?;
    Ok(json!({ "on": on }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::Domain;
    use metal_perform::fixture::FixtureConfig;

    /// A scratch `data/`-shaped dir, like `save_graph_at`/`load_graph_at`'s
    /// own tests use, so this module never touches the repo's real
    /// `data/graphs/` — the testing convention this worktree already
    /// learned the hard way on the switches lote.
    fn scratch() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fw-blackout-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        metal_vfs::create_dir_all(dir.join("graphs")).unwrap();
        dir
    }

    fn device_graph(id: &str) -> Graph {
        let mut g = Graph::new(id, "t", Domain::Live);
        g.add_node("out", "output.device", [0.0, 0.0]);
        g
    }

    /// The test `S.O.1` names verbatim: blackout must not only `bypass`
    /// screens, it must also write the explicit-black `forced` fact onto
    /// every `out.*` fixture — a fixture apagado no es un fixture en negro.
    /// This would fail if `apply_at` forgot the fixture half and only
    /// flipped `bypass`.
    #[test]
    fn blackout_blacks_fixtures_not_only_screens() {
        let root = scratch();
        let mut g = device_graph("device-probe");
        g.add_node("fixture", "out.sacn", [1.0, 0.0]);
        metal_perform::engine::save_graph_at(&root, &g).unwrap();

        apply_at(&root, true).unwrap();

        let reloaded = metal_perform::engine::load_graph_at(&root, "device-probe").unwrap();
        let screen = reloaded
            .nodes
            .iter()
            .find(|n| n.kind == "output.device")
            .unwrap();
        assert!(screen.bypass, "the screen is bypassed");

        let fixture = reloaded
            .nodes
            .iter()
            .find(|n| n.kind == "out.sacn")
            .unwrap();
        assert!(
            !fixture.bypass,
            "a fixture is not bypassed by blackout — bypass only stops rendering, \
             it does nothing to what is already on the wire"
        );
        let cfg = FixtureConfig::from_node(fixture);
        assert_eq!(
            cfg.forced,
            Some([0.0, 0.0, 0.0]),
            "the fixture must carry an explicit black, not just a bypass"
        );

        // And both facts reach what is actually sent, read back off the
        // saved graph the way the render path reads it (`S.O`, 20-sept):
        // the screen compiles to nothing (`overlay::screen_off` is what
        // `compile_device_plan` asks), and the head's level byte is zero
        // where the wire asked for full, with the beam left where it was.
        assert!(
            metal_perform::overlay::screen_off(&reloaded),
            "the render path sees the screen as off"
        );
        metal_vfs::create_dir_all(metal_perform::fixture::dir(&root)).unwrap();
        metal_vfs::write(
            metal_perform::fixture::dir(&root).join("probe.toml"),
            "[[channel]]\nsignal = \"level\"\ncoarse = 0\n\n[[channel]]\nsignal = \"pan\"\ncoarse = 1\n",
        )
        .unwrap();
        let head = metal_perform::fixture::Head {
            fixture: "probe".into(),
            universe: None,
            address: None,
        };
        let wire = metal_schema::Signal {
            level: 1.0,
            hue: 0.0,
            sat: 0.0,
            strobe: 0.0,
            pan: 1.0,
            tilt: 0.0,
            a: 0.0,
            b: 0.0,
            cct: 5600.0,
            zoom: 0.0,
            focus: 0.0,
            iris: 0.0,
            frost: 0.0,
        };
        let slots = metal_perform::fixture::drive_head(&root, &head, Some(&reloaded), None, &wire)
            .expect("the probe profile");
        assert_eq!(slots, vec![(1, 0), (2, 255)], "level black, pan untouched");

        metal_vfs::remove_dir_all(&root).ok();
    }

    /// `S.AL.1`: the byte `blackout_blacks_fixtures_not_only_screens` checks
    /// against `drive_head`'s own `Vec<(u16, u8)>` is not just an
    /// intermediate value any more — it is what actually lands in an E1.31
    /// packet on the wire. Same scratch graph and forced-black fixture as
    /// that test, one more step past `drive_head`: the slots it returns go
    /// through `metal_perform::wire::Sacn` at a loopback socket this test binds
    /// itself, and the level byte is read back out of the packet at its own
    /// DMX offset (126 bytes of fixed E1.31 header, then the address's own
    /// slot — `wire`'s own module doc has the byte-for-byte layout).
    #[test]
    fn blackout_black_reaches_an_actual_e131_packet() {
        let root = scratch();
        let mut g = device_graph("device-probe3");
        g.add_node("fixture", "out.sacn", [1.0, 0.0]);
        metal_perform::engine::save_graph_at(&root, &g).unwrap();

        apply_at(&root, true).unwrap();
        let reloaded = metal_perform::engine::load_graph_at(&root, "device-probe3").unwrap();

        metal_vfs::create_dir_all(metal_perform::fixture::dir(&root)).unwrap();
        metal_vfs::write(
            metal_perform::fixture::dir(&root).join("probe.toml"),
            "[[channel]]\nsignal = \"level\"\ncoarse = 0\n\n[[channel]]\nsignal = \"pan\"\ncoarse = 1\n",
        )
        .unwrap();
        let head = metal_perform::fixture::Head {
            fixture: "probe".into(),
            universe: None,
            address: None,
        };
        let wire_signal = metal_schema::Signal {
            level: 1.0,
            hue: 0.0,
            sat: 0.0,
            strobe: 0.0,
            pan: 1.0,
            tilt: 0.0,
            a: 0.0,
            b: 0.0,
            cct: 5600.0,
            zoom: 0.0,
            focus: 0.0,
            iris: 0.0,
            frost: 0.0,
        };
        let slots =
            metal_perform::fixture::drive_head(&root, &head, Some(&reloaded), None, &wire_signal)
                .expect("the probe profile");
        assert_eq!(
            slots,
            vec![(1, 0), (2, 255)],
            "same forced-black bytes as the sibling test"
        );

        // The wire half: a loopback receiver, and the exact same slots on an
        // actual E1.31 packet — not the intermediate `Vec` any more.
        let recv = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        recv.set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();
        let port = recv.local_addr().unwrap().port();
        use metal_perform::wire::Wire as _;
        let mut sacn =
            metal_perform::wire::Sacn::new(Some(format!("127.0.0.1:{port}").parse().unwrap()))
                .expect("a loopback socket must bind");
        assert!(
            sacn.send(1, &slots),
            "send to a bound local receiver must not fail"
        );

        let mut buf = [0u8; 700];
        let (n, _) = recv.recv_from(&mut buf).expect("a packet must arrive");
        assert_eq!(n, 638);
        assert_eq!(
            &buf[4..16],
            b"ASC-E1.17\0\0\0",
            "root layer's ACN packet identifier"
        );
        assert_eq!(
            u16::from_be_bytes([buf[113], buf[114]]),
            1,
            "the universe field"
        );
        // 126 bytes of fixed header, then slot (address - 1).
        for (address, byte) in &slots {
            assert_eq!(
                buf[126 + (*address as usize - 1)],
                *byte,
                "address {address} must carry the same byte drive_head produced"
            );
        }

        metal_vfs::remove_dir_all(&root).ok();
    }

    /// A configured fixture comes back from a blackout exactly as it went
    /// in: `forced` is the only key blackout writes, so the wire's own
    /// settings (a universe, a unicast target, a frame-driven node's heads)
    /// are not lost on the way.
    #[test]
    fn blackout_keeps_a_fixtures_own_config() {
        let root = scratch();
        let mut g = device_graph("device-probe4");
        let out = g.add_node("fixture", "out.sacn", [1.0, 0.0]);
        let wired = r#"{"heads":[{"fixture":"example-wash","sample":[0.5,0.5]}],"unicast":"10.0.0.9","universe":7}"#;
        g.node_mut(&out).unwrap().config = metal_graph::Config::from_json_str(wired).unwrap();
        metal_perform::engine::save_graph_at(&root, &g).unwrap();

        apply_at(&root, true).unwrap();
        let dark = metal_perform::engine::load_graph_at(&root, "device-probe4").unwrap();
        let node = dark.node(&out).unwrap();
        assert_eq!(FixtureConfig::from_node(node).forced, Some([0.0, 0.0, 0.0]));
        assert_eq!(metal_perform::wire::SacnConfig::from_node(node).universe, 7);

        apply_at(&root, false).unwrap();
        let back = metal_perform::engine::load_graph_at(&root, "device-probe4").unwrap();
        assert_eq!(back.node(&out).unwrap().config.to_json_str(), wired);

        metal_vfs::remove_dir_all(&root).ok();
    }

    #[test]
    fn releasing_blackout_clears_both_halves() {
        let root = scratch();
        let mut g = device_graph("device-probe2");
        g.add_node("fixture", "out.sacn", [1.0, 0.0]);
        metal_perform::engine::save_graph_at(&root, &g).unwrap();

        apply_at(&root, true).unwrap();
        apply_at(&root, false).unwrap();

        let reloaded = metal_perform::engine::load_graph_at(&root, "device-probe2").unwrap();
        let screen = reloaded
            .nodes
            .iter()
            .find(|n| n.kind == "output.device")
            .unwrap();
        assert!(!screen.bypass, "released — the screen is back");
        let fixture = reloaded
            .nodes
            .iter()
            .find(|n| n.kind == "out.sacn")
            .unwrap();
        assert_eq!(
            FixtureConfig::from_node(fixture).forced,
            None,
            "released — nothing forced any more"
        );

        metal_vfs::remove_dir_all(&root).ok();
    }

    #[test]
    fn is_blacked_out_reads_fresh_off_the_graphs_not_a_stored_bit() {
        let root = scratch();
        metal_perform::engine::save_graph_at(&root, &device_graph("device-a")).unwrap();
        metal_perform::engine::save_graph_at(&root, &device_graph("device-b")).unwrap();

        assert!(!is_blacked_out_at(&root), "nothing bypassed yet");
        apply_at(&root, true).unwrap();
        assert!(is_blacked_out_at(&root), "every device now bypassed");

        // Flip one device back by hand, outside blackout entirely — the
        // very next read must see it, regla 4.
        let mut a = metal_perform::engine::load_graph_at(&root, "device-a").unwrap();
        a.nodes
            .iter_mut()
            .find(|n| n.kind == "output.device")
            .unwrap()
            .bypass = false;
        metal_perform::engine::save_graph_at(&root, &a).unwrap();
        assert!(!is_blacked_out_at(&root), "one device disagrees now");

        metal_vfs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_devices_anywhere_reads_as_not_blacked_out() {
        let root = scratch();
        assert!(!is_blacked_out_at(&root), "nothing to be on of");
        metal_vfs::remove_dir_all(&root).ok();
    }

    // The two route handlers above (`blackout_get`/`blackout_post`) touch
    // `output::data_root()` — the real ambient `data/` — like every other
    // handler in this crate's `call` modules. Per this worktree's own
    // testing convention (learned on the switches lote), that is not
    // exercised here directly; `apply_at`/`is_blacked_out_at` above (the
    // pure functions the handlers are thin wrappers over) are what carry
    // the coverage, against scratch dirs that never touch the repo's real
    // `data/graphs/`.
}
