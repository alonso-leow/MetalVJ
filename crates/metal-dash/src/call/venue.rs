//! 3D venues (`metal_perform::venue`): list, check and test-project an
//! AI-authored venue.
//!
//! `POST /venue { op: "project" }` is the only write, and it writes nothing
//! new: it compiles the venue to an `Escena`, projects it through the chosen
//! projector, and puts the result into `Output.masks`, the same list
//! `POST /outputs { op: "mask" }` edits. From there the device cuts it like
//! any hand-drawn contour. The test projection itself goes through two
//! existing routes (`POST /overlay { op: "solid" }` for the colour,
//! `POST /monitor { op: "open" }`, the `open monitor` button, for the
//! screen), which is what the `project_venue` tool chains.

use serde_json::{json, Value};

use glam::Vec3;

use crate::call::{CallError, Ctx, Params};
use crate::output::Outputs;
use crate::venue::{self, Venue};

/// `GET /venue`: every venue on disk with its check counts, or with `?id=`
/// one venue, its full report and the masks each of its projectors would get.
pub fn list(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let root = crate::output::data_root();
    // D3: the universe bus's live merged state, a fresh reading every call
    // (rule 4) — what `POST /outputs {op:"dmx-bus"}` configures.
    let dmx_bus = ctx.state.dmx_bus.snapshot_now(128);
    if let Some(id) = params.param("id") {
        // `last`: the venue projected most recently (`venue::remember`),
        // which is what `look_venue` reads when it is not told which.
        let id = match id {
            "last" => venue::last(&root).ok_or_else(|| {
                CallError::Bad("no venue has been projected yet: name one with `id`".into())
            })?,
            other => other.to_string(),
        };
        let id = id.as_str();
        let v = venue::load(&root, id).map_err(CallError::Bad)?;
        let report = v.check();
        let outputs: Vec<Value> = v
            .outputs()
            .iter()
            .map(|o| {
                json!({
                    "output": o,
                    "projector": v.projectors.iter().find(|p| &p.output == o).map(|p| &p.id),
                    "display": v.displays.iter().find(|d| &d.output == o).map(|d| &d.id),
                    "masks": v.project(o).unwrap_or_default().iter()
                        .map(|m| json!({ "id": m.id, "points": m.points }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        let spans: Vec<Value> = v
            .compositions
            .iter()
            .map(|c| json!({ "composition": c.id, "span": v.spans(c).unwrap_or_default() }))
            .collect();
        // M4.3: the patch table — each fixture's wire, universe, address and
        // channel span, computed from its profile's own channels (the panel
        // flags the overlaps). A profile-less fixture spans nothing.
        let patch: Vec<Value> = v
            .fixtures
            .iter()
            .map(|f| {
                let profile = f
                    .profile
                    .as_deref()
                    .and_then(|m| crate::fixture::get(&root, m));
                let profile_span = profile
                    .as_ref()
                    .and_then(|p| {
                        p.channels
                            .iter()
                            .flat_map(|c| c.coarse.into_iter().chain(c.fine))
                            .max()
                            .map(|last| last + 1)
                    })
                    .unwrap_or(0);
                // D4: a matrix spans its own channel map; its start is the
                // fixture's own address (a matrix has no profile).
                let (span, universe, address) = match &f.matrix {
                    Some(m) => (
                        (m.top() - m.base() + 1).max(0) as u16,
                        f.universe.unwrap_or(0),
                        f.address.unwrap_or(1),
                    ),
                    None => (
                        profile_span,
                        f.universe
                            .or(profile.as_ref().map(|p| p.universe))
                            .unwrap_or(0),
                        f.address
                            .or(profile.as_ref().map(|p| p.address))
                            .unwrap_or(0),
                    ),
                };
                json!({
                    "fixture": f.id,
                    "kind": f.kind.word(),
                    "wire": f.wire,
                    "universe": universe,
                    "address": address,
                    "span": span,
                })
            })
            .collect();
        let draft = load_draft(&root, id);
        let blend = blend_report(&v);
        return Ok(
            json!({ "venue": v, "report": report, "outputs": outputs, "compositions": spans, "patch": patch, "draft": draft, "blend": blend, "dmx_bus": dmx_bus }),
        );
    }
    let venues: Vec<Value> = venue::list(&root)
        .iter()
        .map(|id| match venue::load(&root, id) {
            Ok(v) => {
                let r = v.check();
                json!({
                    "id": id,
                    "name": v.venue.name,
                    "setting": v.venue.setting,
                    "shots": v.shots.len(),
                    "masks": v.masks.len(),
                    "errors": r.errors(),
                    "warnings": r.warnings(),
                })
            }
            Err(e) => json!({ "id": id, "error": e }),
        })
        .collect();
    Ok(json!({ "venues": venues, "last": venue::last(&root), "dmx_bus": dmx_bus }))
}

/// `GET /fixtures`: the fixture profiles under `data/fixtures/` (M4.2) — the
/// names the venue editor's profile combo offers.
pub fn fixtures_list(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let root = crate::output::data_root();
    let loaded = crate::fixture::load(&root);
    let models: Vec<String> = loaded.iter().map(|f| f.model.clone()).collect();
    // F6: the selectors each profile declares, keyed by model, so a fixture
    // page can lay its lane out in columns. Additive: `fixtures` stays the
    // list of model names every existing caller reads.
    let selectors: serde_json::Map<String, Value> = loaded
        .iter()
        .map(|f| {
            (
                f.model.clone(),
                Value::Array(
                    f.selectors()
                        .iter()
                        .map(|s| Value::String(s.name().to_string()))
                        .collect(),
                ),
            )
        })
        .collect();
    Ok(json!({ "fixtures": models, "selectors": selectors }))
}

/// F10/F11/F14: the last packet one `venue.fixture` node sent this tick, read
/// off the universe bus — its own shadow copy, the colour that is actually on
/// the wire, **never recomputed** — with a swatch decoded from the fixture
/// profile's own channels ([`crate::fixture::swatch_of`]).
/// `GET /venue/fixture-packet?graph=<gid>&node=<id>`.
///
/// `state` is `bypassed` (the node is off), `orphan` (its venue row or profile
/// is gone), `sending` (a live packet is on the bus) or `silent` (the node is
/// live but nothing was sent). A panel greys a bypassed or orphan node.
pub fn fixture_packet(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let root = crate::output::data_root();
    let Some(gid) = params.param("graph") else {
        return Err(CallError::Bad("which graph".into()));
    };
    let Some(node_id) = params.param("node") else {
        return Err(CallError::Bad("which node".into()));
    };
    let g = crate::load_graph(gid).map_err(CallError::Bad)?;
    let node = g
        .node(node_id)
        .ok_or_else(|| CallError::Bad(format!("no such node: {node_id}")))?;
    if node.kind != metal_graph::VENUE_FIXTURE_KIND {
        return Err(CallError::Bad(format!(
            "{node_id} is not a venue.fixture node"
        )));
    }
    let prefix = format!("venue/{gid}/{node_id}/");
    Ok(fixture_packet_json(
        &root,
        &ctx.state.dmx_bus,
        node,
        &prefix,
    ))
}

/// The body of [`fixture_packet`], split from the route so a test can point it
/// at a bus and a tempdir (rule 8). Reads the packet; never recomputes the
/// colour.
pub(crate) fn fixture_packet_json(
    root: &std::path::Path,
    bus: &metal_perform::dmx_bus::DmxBus,
    node: &metal_graph::Node,
    prefix: &str,
) -> Value {
    let cfg = metal_perform::wire::VenueFixtureConfig::from_node(node);
    let row = metal_perform::venue::load(root, &cfg.venue)
        .ok()
        .and_then(|v| v.fixtures.into_iter().find(|f| f.id == cfg.fixture));
    let profile = row
        .as_ref()
        .and_then(|f| f.profile.as_deref())
        .and_then(|m| crate::fixture::get(root, m));
    let (universe, address) = match (&row, &profile) {
        (Some(f), Some(p)) => (
            f.universe.or(Some(p.universe)).unwrap_or(0),
            f.address.or(Some(p.address)).unwrap_or(0),
        ),
        _ => (0, 0),
    };
    let packet = bus.source_packet_for_now(prefix);
    let state = if node.bypass {
        "bypassed"
    } else if row.is_none() || profile.is_none() {
        "orphan"
    } else if packet.is_some() {
        "sending"
    } else {
        "silent"
    };
    let colour = match (profile.as_ref(), &packet) {
        (Some(p), Some((_, slots))) => crate::fixture::swatch_of(p, address, slots),
        _ => [0.0, 0.0, 0.0],
    };
    json!({
        "node": node.id,
        "state": state,
        "venue": cfg.venue,
        "fixture": cfg.fixture,
        "name": row.as_ref().map(|f| f.name.clone()).unwrap_or_default(),
        "kind": row.as_ref().map(|f| f.kind.word()).unwrap_or_default(),
        "profile": row.as_ref().and_then(|f| f.profile.clone()),
        "universe": universe,
        "address": address,
        "colour": [colour[0], colour[1], colour[2]],
    })
}

/// `POST /fixtures { op: "import-gdtf", path|bytes, name?, mode? }` (D2):
/// import a `.gdtf` into `data/fixtures/<model>.toml` and answer
/// `{ok, model, channels}`. The one write on this route; the read is
/// `GET /fixtures`. The AI's `import_gdtf` tool posts the same body (rule 6).
pub fn fixtures_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params
        .json()
        .ok_or_else(|| CallError::Bad("expected json".into()))?;
    match body["op"].as_str().unwrap_or_default() {
        "import-gdtf" => fixtures_import(&crate::output::data_root(), &body),
        other => Err(CallError::Bad(format!(
            "unknown op {other:?} (POST /fixtures knows \"import-gdtf\")"
        ))),
    }
}

/// The import, split from the route so a test can point it at a tempdir
/// (rule 8). `path` reads a `.gdtf` off disk; `bytes` is the whole file as
/// base64, for a caller with no shared filesystem. `name` overrides the
/// model id, `mode` picks a DMX mode by name or index.
pub(crate) fn fixtures_import(root: &std::path::Path, body: &Value) -> Result<Value, CallError> {
    let name = body["name"].as_str();
    let mode = body["mode"].as_str();
    let fixture = match (body["path"].as_str(), body["bytes"].as_str()) {
        (Some(path), _) => metal_perform::gdtf::import_path(std::path::Path::new(path), name, mode),
        (None, Some(b64)) => decode_base64(b64)
            .and_then(|bytes| metal_perform::gdtf::import_bytes(&bytes, name, mode)),
        (None, None) => {
            return Err(CallError::Bad(
                "import-gdtf needs a `path` or `bytes`".into(),
            ))
        }
    }
    .map_err(CallError::Bad)?;
    metal_perform::gdtf::save(root, &fixture).map_err(CallError::Bad)?;
    Ok(json!({ "ok": true, "model": fixture.model, "channels": fixture.channels.len() }))
}

/// Standard base64 decode (the same alphabet `metal-agent`'s own encoder
/// uses), so a caller can hand the whole `.gdtf` as a `bytes` string rather
/// than a filesystem path. Whitespace and padding are ignored; any other
/// character is an error.
fn decode_base64(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|&b| b != b'=' && !b.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let vals: Vec<u8> = chunk
            .iter()
            .map(|&b| val(b).ok_or_else(|| format!("invalid base64 byte: {b}")))
            .collect::<Result<_, _>>()?;
        let n = vals.iter().fold(0u32, |acc, &v| (acc << 6) | v as u32) << (6 * (4 - vals.len()));
        out.push((n >> 16) as u8);
        if vals.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if vals.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// `GET /venue/state`: every fixture's live state — what the show commanded
/// and what the head has actually reached (M2). With no `id`, the venue
/// projected last. A state is initialised from the authored pose; the slew
/// model (`FixtureState::step`) is what a running show moves it with.
pub fn state(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let root = crate::output::data_root();
    let id = match params.param("id") {
        Some("last") | None => venue::last(&root).ok_or_else(|| {
            CallError::Bad("no venue has been projected yet: pass `?id=<venue>`".into())
        })?,
        Some(other) => other.to_string(),
    };
    let v = venue::load(&root, &id).map_err(CallError::Bad)?;
    let states = v.states();
    Ok(json!({ "venue": id, "fixtures": states }))
}

/// `POST /venue { op: "check", id }` or
/// `POST /venue { op: "project", id, device?, projector? }`.
///
/// `project` writes each projector's masks onto its output, ids prefixed
/// `<venue>.` so a second projection replaces the first and leaves the
/// operator's own masks alone. `device` sends one projector (`projector`, or
/// the first) to a device other than the one the venue names, which is how
/// an example is tried on whatever projector is plugged in.
pub fn venue(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let root = crate::output::data_root();
    let op = body["op"].as_str().unwrap_or_default();
    // `last` (or no venue) is the venue projected most recently, the same
    // convention `GET /venue?id=last` follows, so `aim` can name only the
    // fixture. The fixtures op names its venue with `venue` (the AI tool's
    // own argument) and its fixture with `id`; every other op names the venue
    // with `id`.
    let named = if op == "fixture" {
        body["venue"].as_str().or_else(|| body["id"].as_str())
    } else {
        body["id"].as_str()
    };
    let id = match named {
        Some("last") | None => venue::last(&root).ok_or_else(|| {
            CallError::Bad("no venue has been projected yet: name one with `id`".into())
        })?,
        Some(other) => other.to_string(),
    };
    let mut v = room_for(&root, &id, op).map_err(CallError::Bad)?;
    let report = v.check();
    match op {
        "check" => Ok(json!({ "id": id, "report": report })),
        // H5.3: place or update a capture camera (`Venue.sensors`). The
        // panel's `venue sensor` folder posts this; the sensor keeps its pose
        // and names the `capture` node's source, and [`venue::save`] writes
        // the file the `sensor.yolo` runtime reads.
        "sensor" => {
            let Some(sensor) = body["sensor"].as_object() else {
                return Err(CallError::Bad("expected `sensor` (an object)".into()));
            };
            let s: venue::Sensor = serde_json::from_value(Value::Object(sensor.clone()))
                .map_err(|e| CallError::Bad(format!("sensor: {e}")))?;
            match v.sensors.iter_mut().find(|x| x.id == s.id) {
                Some(existing) => *existing = s,
                None => v.sensors.push(s),
            }
            venue::save(&root, &v).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "sensors": v.sensors }))
        }
        // P3: the three authoring ops. Same body from the shell and from the
        // agent (rule 6), and each writes through `Venue::save`.
        "surface" => {
            edit_surface(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "venue": v }))
        }
        "pins" => {
            set_pins(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "venue": v }))
        }
        // P-F: edge blending — `blend { enabled, gamma, curve, black_lift }`
        // on one `projector`, or on every projector pinned to `surface` (the
        // editor's "Blend edges" toggle and the `blend_edges` tool).
        "blend" => {
            let changed = set_blend(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "projectors": changed, "blend": blend_report(&v) }))
        }
        // The venue a graph uses when a `venue.surface` names none: the same
        // `venue::last` `look_venue` reads (`data/venues/last`).
        "active" => {
            venue::remember(&root, &id).map_err(|e| CallError::Bad(e.to_string()))?;
            Ok(json!({ "ok": true, "id": id }))
        }
        "project" => {
            if report.errors() > 0 {
                return Err(CallError::Bad(
                    json!({ "error": "the venue does not check", "report": report }).to_string(),
                ));
            }
            if body["device"].is_string() || body["projector"].is_string() {
                retarget(&mut v, body["device"].as_str(), body["projector"].as_str())
                    .map_err(CallError::Bad)?;
            }
            if body["add_outputs"].as_bool().unwrap_or(false) {
                add_outputs(ctx, &v);
            }
            let devices = project(ctx, &v)?;
            let sources = compose(ctx, &v)?;
            let fixtures = fixtures(&root, &v)?;
            if let Err(e) = venue::remember(&root, &id) {
                eprintln!("  last venue not remembered: {e}");
            }
            Ok(
                json!({ "ok": true, "id": id, "devices": devices, "sources": sources,
                       "fixtures": fixtures, "report": report }),
            )
        }
        // M2: aim a head at a point. Inverse kinematics from its pose, within
        // the profile's limits and by the shortest path; the returned pan/tilt
        // is the live aim (`GET /venue/state` records it beside the authored
        // pose).
        "aim" => {
            let fixture = body["fixture"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which fixture: `fixture`".into()))?;
            let target = parse_target(&body["target"]).ok_or_else(|| {
                CallError::Bad("aim needs a `target`: [x, y, z] in metres".into())
            })?;
            let f =
                v.fixtures.iter().find(|f| f.id == fixture).ok_or_else(|| {
                    CallError::Bad(format!("no fixture {fixture:?} in this venue"))
                })?;
            let limits = f
                .profile
                .as_deref()
                .and_then(|m| crate::fixture::get(&root, m))
                .map(|p| p.limits)
                .unwrap_or_default();
            let (pan, tilt) = v
                .aim_with_limits(f, target, &limits)
                .map_err(CallError::Bad)?;
            let hit = v
                .hit(Vec3::from(f.at.unwrap_or([0.0; 3])), venue::direction(pan, tilt))
                .map(|h| json!({ "what": h.what, "at": h.at, "distance": h.distance, "normal": h.normal }));
            Ok(
                json!({ "ok": true, "venue": id, "fixture": fixture, "pan": pan, "tilt": tilt, "hit": hit }),
            )
        }
        // D5: solve a head's pan/tilt so its beam points at a real 3D point,
        // from wherever it is hung (within the profile's limits and slew),
        // and report the live state and where the beam lands. The same body
        // the `solve_head` agent tool posts (rule 6).
        "solve_head" => solve_head_reply(&root, &v, &id, &body),
        // M4.1: place a palette entry at a point and save the venue.
        "place" => {
            let kind = body["kind"]
                .as_str()
                .ok_or_else(|| CallError::Bad("place needs a `kind`".into()))?;
            let at = parse_target(&body["at"])
                .ok_or_else(|| CallError::Bad("place needs an `at`: [x, y, z] in metres".into()))?;
            let placed = v.place_entry(kind, at).map_err(CallError::Bad)?;
            venue::save(&root, &v).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "venue": id, "placed": placed }))
        }
        // M4.1: an LED polyline drawn with the shared contour editor.
        "path" => {
            let fixture = body["fixture"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which fixture: `fixture`".into()))?;
            let path = parse_path(&body["path"])
                .ok_or_else(|| CallError::Bad("path needs a `path`: [[s, t], …]".into()))?;
            let f = v
                .fixtures
                .iter_mut()
                .find(|f| f.id == fixture)
                .ok_or_else(|| CallError::Bad(format!("no fixture {fixture:?} in this venue")))?;
            f.path = path;
            venue::save(&root, &v).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "venue": id, "fixture": fixture }))
        }
        // D1: add, edit or remove one fixture of the venue — the write the
        // Settings "Fixtures" page and the `set_fixture` tool share (rule 6).
        // The body names its venue with `venue` and its fixture with `id`
        // (absent adds one); `delete: true` removes it. The venue is saved
        // through `Venue::save`, the one venue write path.
        "fixture" => {
            let fixture = edit_fixture(&mut v, &root, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "fixture": fixture }))
        }
        // M4.2: one fixture's profile, group, wire, universe or address.
        "config" => {
            let fixture = body["fixture"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which fixture: `fixture`".into()))?;
            let f = v
                .fixtures
                .iter_mut()
                .find(|f| f.id == fixture)
                .ok_or_else(|| CallError::Bad(format!("no fixture {fixture:?} in this venue")))?;
            if let Some(p) = body.get("profile") {
                f.profile = p.as_str().map(str::to_string);
            }
            if let Some(g) = body.get("group") {
                f.group = g.as_str().map(str::to_string);
            }
            if let Some(w) = body["wire"].as_str() {
                f.wire = w.to_string();
            }
            if let Some(u) = body["universe"].as_u64() {
                f.universe = Some(u as u16);
            }
            if let Some(a) = body["address"].as_u64() {
                f.address = Some(a as u16);
            }
            if let Some(y) = body["yaw"].as_f64() {
                f.yaw = y as f32;
            }
            if let Some(p) = body["pitch"].as_f64() {
                f.pitch = p as f32;
            }
            if let Some(r) = body["roll"].as_f64() {
                f.roll = r as f32;
            }
            venue::save(&root, &v).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "venue": id, "fixture": fixture }))
        }
        // M4.2/M6.3: flash a fixture so the operator (or a camera) can see
        // where it is. Nothing is sent to real hardware from here; the reply
        // is the fixture's footprint and a full-white test colour. With no
        // `fixture`, every fixture of the venue is identified.
        "identify" => {
            let wanted = body["fixture"].as_str();
            let mut found = Vec::new();
            for f in v
                .fixtures
                .iter()
                .filter(|f| wanted.is_none_or(|w| f.id == w))
            {
                let footprint = v.footprint(f).map_err(CallError::Bad)?;
                found.push(
                    json!({ "fixture": f.id, "rgb": [255, 255, 255], "footprint": footprint }),
                );
            }
            if let Some(w) = wanted {
                if found.is_empty() {
                    return Err(CallError::Bad(format!("no fixture {w:?} in this venue")));
                }
            }
            Ok(json!({ "ok": true, "venue": id, "identify": found }))
        }
        // M5: the AI writes a draft venue; the operator accepts or rejects it
        // in the editor's diff view.
        "draft" => {
            let text = match &body["venue"] {
                Value::String(s) => s.clone(),
                other @ Value::Object(_) => {
                    toml::to_string_pretty(other).map_err(|e| CallError::Bad(e.to_string()))?
                }
                _ => {
                    return Err(CallError::Bad(
                        "draft needs a `venue` (TOML text or an object)".into(),
                    ))
                }
            };
            let mut proposed = venue::Venue::parse(&text).map_err(CallError::Bad)?;
            proposed.venue.id = id.clone();
            save_draft(&root, &id, &proposed).map_err(CallError::Bad)?;
            let diff = diff_rows(&v, &proposed);
            Ok(json!({ "ok": true, "venue": id, "draft": { "venue": proposed, "diff": diff } }))
        }
        "accept" => {
            let proposed = load_draft(&root, &id)
                .ok_or_else(|| CallError::Bad(format!("no draft for venue {id:?}")))?;
            venue::save(&root, &proposed).map_err(CallError::Bad)?;
            remove_draft(&root, &id);
            Ok(json!({ "ok": true, "venue": id, "accepted": true }))
        }
        "reject" => {
            remove_draft(&root, &id);
            Ok(json!({ "ok": true, "venue": id, "rejected": true }))
        }
        // Venue 3D authoring: objects, screens, projectors, swept forms and
        // free movement. Every write saves through `Venue::save`, and every
        // body is the same one the shell and the agent post (rule 6).
        "object" => {
            let object = edit_object(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "object": object }))
        }
        "stage" | "truss" | "dummy" => {
            let made = object_op_body(&body, op);
            let object = edit_object(&mut v, &made).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "object": object }))
        }
        "boolean" => boolean_op(&mut v, &root, &body),
        "hit" => {
            let origin = parse_target(&body["origin"])
                .ok_or_else(|| CallError::Bad("`origin` is [x, y, z]".into()))?;
            let dir = parse_target(&body["dir"])
                .ok_or_else(|| CallError::Bad("`dir` is [x, y, z]".into()))?;
            let dir = Vec3::from(dir).normalize_or_zero();
            if dir.length_squared() < 1.0e-9 {
                return Err(CallError::Bad("`dir` must not be zero".into()));
            }
            let max = body["max"].as_f64().map(|m| m as f32).unwrap_or(f32::INFINITY);
            let hit = v.hit(Vec3::from(origin), dir).filter(|h| h.distance <= max).map(|h| {
                json!({
                    "what": h.what, "at": h.at, "distance": h.distance,
                    "normal": h.normal,
                })
            });
            Ok(json!({ "ok": true, "id": id, "hit": hit }))
        }
        "report" => {
            let warnings: Vec<Value> = v
                .displays
                .iter()
                .filter_map(|d| {
                    let s = v.surfaces.iter().find(|s| s.id == d.surface)?;
                    let physical = Vec3::from(s.u).length() / Vec3::from(s.v).length();
                    let digital = d.resolution[0] as f32 / d.resolution[1].max(1) as f32;
                    ((physical - digital).abs() > 1.0e-3).then(|| {
                        json!({
                            "display": d.id, "surface": d.surface,
                            "physical_aspect": physical,
                            "resolution_aspect": digital,
                        })
                    })
                })
                .collect();
            Ok(json!({ "ok": true, "id": id, "check": report, "aspect_warnings": warnings }))
        }
        "view" => view_op(&v, &body),
        "screen" => {
            let made = edit_screen(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            add_outputs(ctx, &v);
            Ok(json!({
                "ok": true, "id": id, "display": made.display,
                "surface": made.surface, "output": made.output,
                "aspect_locked": made.aspect_locked,
            }))
        }
        "projector" => {
            let projector = edit_projector(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "projector": projector }))
        }
        "surface.form" => {
            let (surface, planar, bbox) =
                set_surface_form(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({
                "ok": true, "id": id, "surface": surface,
                "is_planar": planar, "bbox": bbox,
            }))
        }
        // P-V: the surface's sector grid — how one `venue.surface` node's many
        // sources share this surface, projected by the projector or drawn by
        // the operator. Upserts onto the surface, saves, and echoes it.
        "surface.sectors" => {
            let surface = set_surface_sectors(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "surface": surface }))
        }
        "move" => {
            let moved = move_item(&mut v, &body).map_err(CallError::Bad)?;
            v.save(&root).map_err(CallError::Bad)?;
            Ok(json!({ "ok": true, "id": id, "moved": moved }))
        }
        // Venue 3D: a mask drawn on a surface in space. Upserts by mask id
        // (or removes it with `delete`), keeping the model's own fields.
        "mask" => {
            let mask_id = body["mask"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CallError::Bad("which mask: `mask`".into()))?
                .to_string();
            if body["delete"].as_bool().unwrap_or(false) {
                v.masks.retain(|m| m.id != mask_id);
            } else {
                let on = body["on"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| CallError::Bad("which surface: `on`".into()))?
                    .to_string();
                let points = parse_path(&body["points"])
                    .ok_or_else(|| CallError::Bad("mask needs `points`: [[s, t], …]".into()))?;
                let rgb = match body.get("rgb") {
                    Some(Value::Null) | None => None,
                    Some(v) => Some(
                        v.as_array()
                            .filter(|a| a.len() == 3)
                            .map(|a| {
                                a.iter()
                                    .map(|x| x.as_u64().unwrap_or(0).min(255) as u8)
                                    .collect::<Vec<u8>>()
                            })
                            .and_then(|a| <[u8; 3]>::try_from(a).ok())
                            .ok_or_else(|| CallError::Bad("`rgb` is [r, g, b]".into()))?,
                    ),
                };
                let alpha = body["alpha"].as_f64().map(|a| a as f32).unwrap_or(1.0);
                let parent = body["parent"].as_str().map(str::to_string);
                let content = body["content"].as_str().unwrap_or_default().to_string();
                match v.masks.iter_mut().find(|m| m.id == mask_id) {
                    Some(m) => {
                        m.on = on;
                        m.points = points;
                        if rgb.is_some() {
                            m.rgb = rgb;
                        }
                        m.parent = parent;
                        m.alpha = alpha;
                        if body.get("content").is_some() {
                            m.content = content;
                        }
                    }
                    None => v.masks.push(venue::VenueMask {
                        id: mask_id.clone(),
                        name: body["name"].as_str().unwrap_or(&mask_id).to_string(),
                        on,
                        points,
                        rgb,
                        content,
                        parent,
                        alpha,
                    }),
                }
            }
            v.save(&root).map_err(CallError::Bad)?;
            let mask = v.masks.iter().find(|m| m.id == mask_id).cloned();
            Ok(json!({ "ok": true, "id": id, "mask": mask }))
        }
        "area" => area_reply(&v, &body),
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

/// The id of the thing an authoring op names, falling back to the spelling
/// older bodies used (`id_object` for an object).
/// The room an op works on. A room with nothing on disk yet is created by its
/// first write (the gallery's starter, or the first add) instead of a 400; a
/// read, or a file that exists but does not parse, is still refused, and a
/// file is never overwritten here.
fn room_for(root: &std::path::Path, id: &str, op: &str) -> Result<Venue, String> {
    match venue::load(root, id) {
        Ok(v) => Ok(v),
        Err(e)
            if creates_room(op)
                && !id.is_empty()
                && !id.contains(['/', '\\'])
                && !id.starts_with('.')
                && !metal_vfs::exists(&venue::dir(root).join(format!("{id}.toml"))) =>
        {
            Ok(blank_room(id))
        }
        Err(e) => Err(e),
    }
}

/// The writes that may create a room that is not on disk yet.
fn creates_room(op: &str) -> bool {
    matches!(
        op,
        "surface" | "screen" | "object" | "stage" | "truss" | "dummy" | "projector" | "place" | "fixture"
    )
}

/// An empty room under `id`: its name is its id, and nothing else is in it.
fn blank_room(id: &str) -> Venue {
    Venue {
        venue: venue::Meta {
            id: id.to_string(),
            name: id.to_string(),
            setting: venue::Setting::Indoor,
            based_on: Vec::new(),
            notes: String::new(),
        },
        shots: Vec::new(),
        surfaces: Vec::new(),
        objects: Vec::new(),
        projectors: Vec::new(),
        fixtures: Vec::new(),
        masks: Vec::new(),
        displays: Vec::new(),
        compositions: Vec::new(),
        sensors: Vec::new(),
    }
}

fn named_id<'a>(body: &'a Value, key: &str, fallback: &str) -> Option<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            body.get(fallback)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
}

fn parse_strings(v: &Value) -> Option<Vec<String>> {
    Some(
        v.as_array()?
            .iter()
            .map(|x| x.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()?,
    )
}

fn parse_crown(v: &Value) -> Option<venue::Crown> {
    let object = v.as_object()?;
    let center = vec3(object.get("center")?)?;
    let radius = object.get("radius")?.as_f64()? as f32;
    radius
        .is_finite()
        .then_some(venue::Crown { center, radius })
}

/// `op: "object"`: create, edit or delete one venue [`venue::Object`]. The
/// body names its venue with `id` and its object with `object` (or the older
/// `id_object`), so the two ids never collide.
fn edit_object(v: &mut Venue, body: &Value) -> Result<Value, String> {
    let id = named_id(body, "object", "id_object")
        .ok_or("which object: `object` (its id)")?
        .to_string();
    if body["delete"].as_bool().unwrap_or(false) {
        let Some(pos) = v.objects.iter().position(|o| o.id == id) else {
            return Err(format!("no object {id:?} in this venue"));
        };
        let removed = v.objects.remove(pos);
        return serde_json::to_value(removed).map_err(|e| e.to_string());
    }
    let existing = v.objects.iter().position(|o| o.id == id);
    if existing.is_none() {
        v.objects.push(venue::Object {
            id: id.clone(),
            name: id.clone(),
            kind: "box".into(),
            at: [0.0, 0.0, 0.0],
            size: [1.0, 1.0, 1.0],
            yaw: 0.0,
            crown: None,
            occludes: true,
            emits: 0.0,
            faces: Vec::new(),
            description: String::new(),
            form: venue::ObjectForm::Box,
            parent: None,
            attribution: String::new(),
            source: String::new(),
        });
    }
    let o = v
        .objects
        .iter_mut()
        .find(|o| o.id == id)
        .expect("the object was just added");
    if let Some(name) = body["name"].as_str() {
        o.name = name.to_string();
    }
    if let Some(kind) = body["kind"].as_str() {
        o.kind = kind.to_string();
    }
    if body.get("form").is_some() {
        o.form = parse_object_form(&body["form"]).map_err(|e| format!("`form`: {e}"))?;
    }
    if body.get("parent").is_some() {
        o.parent = match &body["parent"] {
            Value::Null => None,
            Value::String(p) if p.is_empty() => None,
            Value::String(p) if p == &id => {
                return Err(format!("an object cannot be its own parent: {id:?}"))
            }
            Value::String(p) => Some(p.clone()),
            _ => return Err("`parent` is a string or null".into()),
        };
    }
    if let Some(attribution) = body["attribution"].as_str() {
        o.attribution = attribution.to_string();
    }
    if let Some(source) = body["source"].as_str() {
        o.source = source.to_string();
    }
    if body.get("at").is_some() {
        o.at = vec3(&body["at"]).ok_or("`at` is [x, y, z]")?;
    }
    if body.get("size").is_some() {
        o.size = vec3(&body["size"]).ok_or("`size` is [w, h, d]")?;
    }
    if let Some(yaw) = body["yaw"].as_f64() {
        o.yaw = yaw as f32;
    }
    match body.get("crown") {
        Some(Value::Null) => o.crown = None,
        Some(v) => o.crown = Some(parse_crown(v).ok_or("`crown` is {center:[x,y,z], radius}")?),
        None => {}
    }
    if let Some(occludes) = body["occludes"].as_bool() {
        o.occludes = occludes;
    }
    if let Some(emits) = body["emits"].as_f64() {
        o.emits = emits as f32;
    }
    if body.get("faces").is_some() {
        o.faces = parse_strings(&body["faces"]).ok_or("`faces` is [string]")?;
    }
    if let Some(description) = body["description"].as_str() {
        o.description = description.to_string();
    }
    serde_json::to_value(&*o).map_err(|e| e.to_string())
}

/// How a screen's aspect is reconciled: the physical width/height wins, the
/// pixels win, or both already agree. Kept pure so the correction is tested
/// without a venue file.
fn correct_aspect(
    width_m: &mut f32,
    height_m: &mut f32,
    pixels: &mut [u32; 2],
    lock: Option<&str>,
) -> bool {
    let physical = *width_m / *height_m;
    let digital = pixels[0] as f32 / pixels[1] as f32;
    if (physical - digital).abs() <= 1.0e-4 {
        return false;
    }
    if lock == Some("physical") {
        pixels[1] = ((pixels[0] as f32 * *height_m / *width_m).round() as u32).max(1);
    } else {
        *height_m = *width_m * pixels[1] as f32 / pixels[0] as f32;
    }
    true
}

/// Where a screen's glass rectangle begins and its two edges, from its centre
/// base point and a yaw about `y`. Pure, so the geometry is tested directly.
fn screen_geometry(
    at: [f32; 3],
    yaw: Option<f32>,
    width_m: f32,
    height_m: f32,
) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let q = glam::Quat::from_rotation_y(-yaw.unwrap_or(0.0).to_radians());
    let origin = Vec3::from(at) + q * Vec3::new(-width_m / 2.0, height_m / 2.0, 0.0);
    let u = q * Vec3::new(width_m, 0.0, 0.0);
    let vv = q * Vec3::new(0.0, -height_m, 0.0);
    (origin.to_array(), u.to_array(), vv.to_array())
}

fn display_kind(word: Option<&str>) -> venue::DisplayKind {
    match word.unwrap_or("monitor") {
        "tv" => venue::DisplayKind::Tv,
        "laptop" => venue::DisplayKind::Laptop,
        "tablet" => venue::DisplayKind::Tablet,
        "phone" => venue::DisplayKind::Phone,
        "led_wall" => venue::DisplayKind::LedWall,
        _ => venue::DisplayKind::Monitor,
    }
}

fn display_link(word: Option<&str>) -> venue::Link {
    match word.unwrap_or("local") {
        "page" => venue::Link::Page,
        "peer" => venue::Link::Peer,
        _ => venue::Link::Local,
    }
}

fn unique_screen_id(v: &Venue) -> String {
    let mut n = v.surfaces.len() + v.displays.len() + 1;
    loop {
        let id = format!("screen-{n}");
        if !v.surfaces.iter().any(|s| s.id == id) && !v.displays.iter().any(|d| d.id == id) {
            return id;
        }
        n += 1;
    }
}

struct ScreenReply {
    display: String,
    surface: String,
    output: String,
    aspect_locked: bool,
}

/// `op: "screen"`: one display, its glass plane and (through `add_outputs`)
/// its output. Aspect correction, surface geometry and the display record are
/// all derived from the one `screen` object the shell and the agent post.
fn edit_screen(v: &mut Venue, body: &Value) -> Result<ScreenReply, String> {
    let screen = body
        .get("screen")
        .and_then(Value::as_object)
        .ok_or("`screen` is an object")?;
    let screen_value = Value::Object(screen.clone());
    let mut width_m = screen_value["width_m"]
        .as_f64()
        .ok_or("`screen.width_m` is a number")? as f32;
    let mut height_m = screen_value["height_m"]
        .as_f64()
        .ok_or("`screen.height_m` is a number")? as f32;
    let mut pixels = match screen_value["pixels"].as_array() {
        Some(a) if a.len() == 2 => [
            a[0].as_u64().ok_or("`screen.pixels` is [width, height]")? as u32,
            a[1].as_u64().ok_or("`screen.pixels` is [width, height]")? as u32,
        ],
        _ => return Err("`screen.pixels` is [width, height]".into()),
    };
    if !(width_m.is_finite() && width_m > 0.0) {
        return Err("`screen.width_m` must be > 0".into());
    }
    if !(height_m.is_finite() && height_m > 0.0) {
        return Err("`screen.height_m` must be > 0".into());
    }
    if pixels[0] == 0 || pixels[1] == 0 {
        return Err("`screen.pixels` entries must be > 0".into());
    }
    let aspect_locked = correct_aspect(
        &mut width_m,
        &mut height_m,
        &mut pixels,
        screen_value["aspect_lock"].as_str(),
    );
    let at = match screen_value.get("at") {
        Some(v) => vec3(v).ok_or("`at` is [x, y, z]")?,
        None => [0.0, 0.0, 0.0],
    };
    let yaw = screen_value["yaw"].as_f64().map(|y| y as f32);
    let surface_id = body["surface"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| unique_screen_id(v));
    let name = screen_value["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&surface_id)
        .to_string();
    let (origin, u, vv) = screen_geometry(at, yaw, width_m, height_m);
    let glass = venue::Surface {
        id: surface_id.clone(),
        name: name.clone(),
        origin,
        u,
        v: vv,
        material: String::new(),
        form: Default::default(),
        sectors: Default::default(),
    };
    match v.surfaces.iter_mut().find(|s| s.id == surface_id) {
        Some(existing) => *existing = glass,
        None => v.surfaces.push(glass),
    }
    let display_id = surface_id.clone();
    let display = venue::Display {
        id: display_id.clone(),
        name: name.clone(),
        kind: display_kind(screen_value["kind"].as_str()),
        output: display_id.clone(),
        surface: surface_id.clone(),
        resolution: pixels,
        link: display_link(screen_value["link"].as_str()),
        node: screen_value["node"].as_str().map(str::to_string),
        notes: screen_value["notes"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    };
    match v.displays.iter_mut().find(|d| d.id == display_id) {
        Some(existing) => *existing = display,
        None => v.displays.push(display),
    }
    Ok(ScreenReply {
        display: display_id.clone(),
        surface: surface_id,
        output: display_id,
        aspect_locked,
    })
}

/// `op: "projector"`: add, edit or remove one [`venue::Projector`]. A
/// projector stands on an object; a surface id is accepted as a fallback the
/// same way `place_entry` falls back to the first surface, and a name that
/// resolves to neither is refused by name.
fn edit_projector(v: &mut Venue, body: &Value) -> Result<Value, String> {
    let id = named_id(body, "projector", "id_projector")
        .ok_or("which projector: `projector` (its id)")?
        .to_string();
    if body["delete"].as_bool().unwrap_or(false) {
        let Some(pos) = v.projectors.iter().position(|p| p.id == id) else {
            return Err(format!("no projector {id:?} in this venue"));
        };
        let removed = v.projectors.remove(pos);
        return serde_json::to_value(removed).map_err(|e| e.to_string());
    }
    let existing = v.projectors.iter().position(|p| p.id == id);
    let on = match body["on"].as_str().filter(|s| !s.is_empty()) {
        Some(on) => {
            if v.objects.iter().any(|o| o.id == on) {
                on.to_string()
            } else if v.surfaces.iter().any(|s| s.id == on) {
                v.objects.first().map(|o| o.id.clone()).ok_or_else(|| {
                    format!("no object to stand projector {id:?} on (named surface {on:?})")
                })?
            } else {
                return Err(format!("no object {on:?} in this venue"));
            }
        }
        None => match existing {
            Some(i) => v.projectors[i].on.clone(),
            None => v
                .objects
                .first()
                .map(|o| o.id.clone())
                .or_else(|| v.surfaces.first().map(|s| s.id.clone()))
                .ok_or("a new projector needs an object (`on`) to stand on")?,
        },
    };
    if existing.is_none() {
        v.projectors.push(venue::Projector {
            id: id.clone(),
            name: id.clone(),
            output: id.clone(),
            on: on.clone(),
            lens: [0.0, 0.12, 0.0],
            yaw: 0.0,
            pitch: -20.0,
            throw_ratio: 1.2,
            aspect: 16.0 / 9.0,
            lumens: None,
            pins: Default::default(),
            blend: Default::default(),
        });
    }
    let p = v
        .projectors
        .iter_mut()
        .find(|p| p.id == id)
        .expect("the projector was just added");
    p.on = on;
    if let Some(name) = body["name"].as_str() {
        p.name = name.to_string();
    }
    if let Some(output) = body["output"].as_str() {
        p.output = output.to_string();
    }
    if body.get("lens").is_some() {
        p.lens = vec3(&body["lens"]).ok_or("`lens` is [x, y, z]")?;
    }
    if let Some(yaw) = body["yaw"].as_f64() {
        p.yaw = yaw as f32;
    }
    if let Some(pitch) = body["pitch"].as_f64() {
        p.pitch = pitch as f32;
    }
    if let Some(throw_ratio) = body["throw_ratio"].as_f64() {
        p.throw_ratio = throw_ratio as f32;
    }
    if let Some(aspect) = body["aspect"].as_f64() {
        p.aspect = aspect as f32;
    }
    match body.get("lumens") {
        Some(Value::Null) => p.lumens = None,
        Some(v) => {
            p.lumens = Some(
                v.as_u64()
                    .ok_or("`lumens` is a number or null")?
                    .min(u32::MAX as u64) as u32,
            )
        }
        None => {}
    }
    serde_json::to_value(&*p).map_err(|e| e.to_string())
}

/// The form a `surface.form` body asks for, parsed before anything is
/// written. Pure so a bad path/profile is refused without touching a venue.
fn surface_form_from_body(body: &Value) -> Result<venue::SurfaceForm, String> {
    match body["form"].as_str() {
        Some("plane") => Ok(venue::SurfaceForm::Plane),
        Some("swept") => {
            let path = body["path"]
                .as_array()
                .ok_or("a swept form needs a `path`")?
                .iter()
                .map(|p| vec3(p).ok_or("a `path` point is [x, y, z]"))
                .collect::<Result<Vec<_>, _>>()?;
            if path.len() < 2 {
                return Err("a swept form needs a `path` of at least 2 points".into());
            }
            let profile = parse_path(&body["profile"])
                .ok_or("a swept form needs a `profile` of [x, y] points")?;
            if profile.len() < 3 {
                return Err("a swept form needs a `profile` of at least 3 points".into());
            }
            let kind = body["kind"].as_str().unwrap_or("catmull_rom").to_string();
            if !matches!(kind.as_str(), "polyline" | "catmull_rom" | "bezier") {
                return Err(format!(
                    "unknown spline kind {kind:?}: polyline, catmull_rom or bezier"
                ));
            }
            Ok(venue::SurfaceForm::Swept {
                path,
                profile,
                caps: body["caps"].as_bool().unwrap_or(false),
                closed: body["closed"].as_bool().unwrap_or(false),
                kind,
            })
        }
        Some(other) => Err(format!("unknown form {other:?}: plane or swept")),
        None => Err("which form: `form` is \"plane\" or \"swept\"".into()),
    }
}

/// `op: "surface.form"`: set or clear a surface's parametric shape.
fn set_surface_form(
    v: &mut Venue,
    body: &Value,
) -> Result<(String, bool, ([f32; 3], [f32; 3])), String> {
    let id = body["surface"]
        .as_str()
        .ok_or("which surface: `surface`")?
        .to_string();
    let form = surface_form_from_body(body)?;
    let s = v
        .surfaces
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("no surface {id:?} in this venue"))?;
    s.form = form;
    if matches!(s.form, venue::SurfaceForm::Plane) {
        if body.get("origin").is_some() {
            s.origin = vec3(&body["origin"]).ok_or("`origin` is [x, y, z]")?;
        }
        if body.get("u").is_some() {
            s.u = vec3(&body["u"]).ok_or("`u` is [x, y, z]")?;
        }
        if body.get("v").is_some() {
            s.v = vec3(&body["v"]).ok_or("`v` is [x, y, z]")?;
        }
    }
    Ok((id, s.is_planar(), s.bbox()))
}

/// `op: "surface.sectors"`: upsert one surface's `sectors` grid. Parsed before
/// anything is written, so a malformed body leaves the venue untouched. Every
/// field is optional — a body with only `{venue, surface, ratio}` keeps the
/// drawn points, and one with only `points` keeps the ratio — so the editor
/// and the AI can each send the half they know.
fn set_surface_sectors(v: &mut Venue, body: &Value) -> Result<venue::Surface, String> {
    let id = body["surface"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("which surface: `surface`")?
        .to_string();
    let source = match body["source"].as_str() {
        None => None,
        Some("projected") => Some(venue::SectorSource::Projected),
        Some("points") => Some(venue::SectorSource::Points),
        Some(other) => return Err(format!("unknown source {other:?}: projected or points")),
    };
    let ratio = match body.get("ratio") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let r = v.as_f64().ok_or("`ratio` is a number")? as f32;
            if !r.is_finite() || !(0.05..=8.0).contains(&r) {
                return Err(format!("`ratio` {r} is not 0.05..=8"));
            }
            Some(r)
        }
    };
    let points = match body.get("points") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            parse_path(v)
                .ok_or("`points` is [[x, y], …] in the output's 0..1")?
                .into_iter()
                .map(|p| {
                    if (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]) {
                        Ok(p)
                    } else {
                        Err(format!("point {p:?} is not inside 0..1"))
                    }
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
    };
    if let Some(points) = &points {
        if points.len() % 2 != 0 {
            return Err("`points` come in pairs: [start, end] per streak".into());
        }
    }
    let magnet = body["magnet"].as_bool();
    let s = v
        .surfaces
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("no surface {id:?} in this venue"))?;
    if let Some(source) = source {
        s.sectors.source = source;
    }
    if let Some(ratio) = ratio {
        s.sectors.ratio = ratio;
    }
    if let Some(points) = points {
        s.sectors.points = points;
    }
    if let Some(magnet) = magnet {
        s.sectors.magnet = magnet;
    }
    Ok(s.clone())
}

/// The translation a move from `current` to `at` applies.
fn move_delta(current: [f32; 3], at: [f32; 3]) -> [f32; 3] {
    [at[0] - current[0], at[1] - current[1], at[2] - current[2]]
}

fn rotate_y(v: [f32; 3], yaw: f32) -> [f32; 3] {
    let q = glam::Quat::from_rotation_y(-yaw.to_radians());
    (q * Vec3::from(v)).to_array()
}

/// `op: "move"`: move any item of the venue by its own vocabulary. A
/// projector refuses `at` because it stands on an object — moving the table
/// is the operator's job.
fn move_item(v: &mut Venue, body: &Value) -> Result<String, String> {
    let id = body["item"]
        .as_str()
        .ok_or("which item: `item` (its id)")?
        .to_string();
    let at = match body.get("at") {
        Some(v) => Some(vec3(v).ok_or("`at` is [x, y, z]")?),
        None => None,
    };
    let yaw = body["yaw"].as_f64().map(|y| y as f32);
    let pitch = body["pitch"].as_f64().map(|p| p as f32);
    let roll = body["roll"].as_f64().map(|r| r as f32);
    if let Some(o) = v.objects.iter_mut().find(|o| o.id == id) {
        if let Some(at) = at {
            o.at = at;
        }
        if let Some(yaw) = yaw {
            o.yaw = yaw;
        }
        return Ok(o.id.clone());
    }
    if let Some(p) = v.projectors.iter_mut().find(|p| p.id == id) {
        if at.is_some() {
            return Err("a projector moves with its object".into());
        }
        if let Some(yaw) = yaw {
            p.yaw = yaw;
        }
        if let Some(pitch) = pitch {
            p.pitch = pitch;
        }
        return Ok(p.id.clone());
    }
    if let Some(f) = v.fixtures.iter_mut().find(|f| f.id == id) {
        if let Some(at) = at {
            f.at = Some(at);
        }
        if let Some(yaw) = yaw {
            f.yaw = yaw;
        }
        if let Some(pitch) = pitch {
            f.pitch = pitch;
        }
        if let Some(roll) = roll {
            f.roll = roll;
        }
        return Ok(f.id.clone());
    }
    if let Some(surface) = v
        .displays
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.surface.clone())
    {
        let s = v
            .surfaces
            .iter_mut()
            .find(|s| s.id == surface)
            .ok_or_else(|| format!("display {id:?} names no surface {surface:?}"))?;
        if let Some(at) = at {
            let d = move_delta(s.origin, at);
            s.origin = [s.origin[0] + d[0], s.origin[1] + d[1], s.origin[2] + d[2]];
        }
        if let Some(yaw) = yaw {
            s.u = rotate_y(s.u, yaw);
            s.v = rotate_y(s.v, yaw);
        }
        return Ok(id);
    }
    if let Some(s) = v.surfaces.iter_mut().find(|s| s.id == id) {
        if let Some(at) = at {
            let d = move_delta(s.origin, at);
            s.origin = [s.origin[0] + d[0], s.origin[1] + d[1], s.origin[2] + d[2]];
        }
        if let Some(yaw) = yaw {
            s.u = rotate_y(s.u, yaw);
            s.v = rotate_y(s.v, yaw);
        }
        return Ok(s.id.clone());
    }
    if let Some(sensor) = v.sensors.iter_mut().find(|s| s.id == id) {
        if let Some(at) = at {
            sensor.at = at;
        }
        if let Some(yaw) = yaw {
            sensor.yaw = yaw;
        }
        if let Some(pitch) = pitch {
            sensor.pitch = pitch;
        }
        if let Some(roll) = roll {
            sensor.roll = roll;
        }
        return Ok(sensor.id.clone());
    }
    Err(format!("no item {id:?} in this venue"))
}

fn wall_mesh(w: &metal_perform::escena::Wall) -> metal_geom::Mesh {
    let o = Vec3::from(w.origin);
    let u = Vec3::from(w.u);
    let vv = Vec3::from(w.v);
    let mut mesh = metal_geom::Mesh::new();
    mesh.positions = vec![
        o.to_array(),
        (o + u).to_array(),
        (o + u + vv).to_array(),
        (o + vv).to_array(),
    ];
    mesh.uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    mesh
}

/// The area a mesh covers in one projector's own 0..1 frame. Triangles with
/// a vertex behind the eye are skipped rather than half-projected.
fn mesh_area(view: &metal_perform::escena::View, mesh: &metal_geom::Mesh) -> f32 {
    let mut total = 0.0;
    for tri in mesh.indices.chunks_exact(3) {
        let mut points = [[0.0f32; 2]; 3];
        let mut ok = true;
        for (i, index) in tri.iter().enumerate() {
            let Some(p) = mesh.positions.get(*index as usize) else {
                ok = false;
                break;
            };
            let Some(q) = view.project(Vec3::from(*p)) else {
                ok = false;
                break;
            };
            points[i] = [q.x, q.y];
        }
        if ok {
            total += metal_geom::query::projected_area(&points);
        }
    }
    total
}

/// `op: "area"`: the live projected area of one projector over every surface
/// and object face, plus how far its axis runs before the first hit.
fn area_reply(v: &Venue, body: &Value) -> Result<Value, CallError> {
    let pid = body["projector"]
        .as_str()
        .ok_or_else(|| CallError::Bad("which projector: `projector`".into()))?;
    let p = v
        .projector(pid)
        .ok_or_else(|| CallError::Bad(format!("no projector {pid:?} in this venue")))?;
    let view = v.view(p).map_err(CallError::Bad)?;
    let lens = v.lens(p).map_err(CallError::Bad)?;
    let mut area_m2 = 0.0;
    let mut surfaces = Vec::new();
    for s in &v.surfaces {
        let a = mesh_area(&view, &s.tessellate(24, 12));
        area_m2 += a;
        surfaces.push(json!({ "id": s.id, "area_m2": a }));
    }
    for o in &v.objects {
        for name in &o.faces {
            let Some(w) = o.face(name) else { continue };
            let a = mesh_area(&view, &wall_mesh(&w));
            area_m2 += a;
            surfaces.push(json!({ "id": w.id, "area_m2": a }));
        }
    }
    let throw_m = v
        .hit(lens, venue::direction(p.yaw, p.pitch))
        .map(|h| h.distance);
    Ok(json!({
        "ok": true, "id": v.venue.id, "projector": pid,
        "area_m2": area_m2, "throw_m": throw_m, "surfaces": surfaces,
    }))
}

/// `GET /venue/scene?id=`: the AI/viewport "see" read. One venue as the
/// editor's items, surfaces, displays, projectors, fixtures, outputs and
/// masks, with no writes.
pub fn scene(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let root = crate::output::data_root();
    let id = match params.param("id") {
        Some("last") | None => venue::last(&root).ok_or_else(|| {
            CallError::Bad("no venue has been projected yet: pass `?id=<venue>`".into())
        })?,
        Some(other) => other.to_string(),
    };
    let v = venue::load(&root, &id).map_err(CallError::Bad)?;
    let mut items = Vec::new();
    for o in &v.objects {
        let form = match &o.form {
            venue::ObjectForm::Box => "box",
            venue::ObjectForm::Swept { .. } => "swept",
            venue::ObjectForm::Mesh { .. } => "mesh",
            venue::ObjectForm::Capsule { .. } => "capsule",
        };
        items.push(json!({
            "kind": "object", "id": o.id, "name": o.name, "at": o.at,
            "size": o.size, "yaw": o.yaw, "pitch": 0.0, "roll": 0.0,
            "form": form, "parent": o.parent,
            "attribution": o.attribution, "source": o.source,
            "bbox": o.bbox(),
        }));
    }
    for f in &v.fixtures {
        items.push(json!({
            "kind": f.kind.word(), "id": f.id, "name": f.name, "at": f.at,
            "size": Value::Null, "yaw": f.yaw, "pitch": f.pitch, "roll": f.roll,
        }));
    }
    for p in &v.projectors {
        items.push(json!({
            "kind": "projector", "id": p.id, "name": p.name,
            "at": v.lens(p).ok().map(|l| l.to_array()), "size": Value::Null,
            "yaw": p.yaw, "pitch": p.pitch, "roll": 0.0,
        }));
    }
    for s in &v.sensors {
        items.push(json!({
            "kind": "sensor", "id": s.id, "name": s.name, "at": s.at,
            "size": Value::Null, "yaw": s.yaw, "pitch": s.pitch, "roll": s.roll,
        }));
    }
    for d in &v.displays {
        let surface = v.surfaces.iter().find(|s| s.id == d.surface);
        let (at, size) = match surface {
            Some(s) => (
                s.origin,
                [Vec3::from(s.u).length(), Vec3::from(s.v).length(), 0.0],
            ),
            None => ([0.0; 3], [0.0; 3]),
        };
        items.push(json!({
            "kind": "display", "id": d.id, "name": d.name, "at": at,
            "size": size, "yaw": 0.0, "pitch": 0.0, "roll": 0.0,
        }));
    }
    let surfaces: Vec<Value> = v
        .surfaces
        .iter()
        .map(|s| {
            json!({
                "id": s.id, "name": s.name, "origin": s.origin, "u": s.u,
                "v": s.v, "material": s.material, "is_planar": s.is_planar(),
                "bbox": s.bbox(),
            })
        })
        .collect();
    Ok(json!({
        "ok": true,
        "venue": { "id": v.venue.id, "name": v.venue.name, "setting": v.venue.setting },
        "items": items,
        "surfaces": surfaces,
        "displays": v.displays,
        "projectors": v.projectors,
        "fixtures": v.fixtures,
        "outputs": v.outputs(),
        "masks": v.masks,
    }))
}

/// `POST /venue { op: "solve_head", fixture, target }`: point a head at a
/// real 3D point and answer its live state plus where the beam lands (D5).
/// Split out of the `venue` match so the op is testable without the whole
/// app's `Ctx`; the agent tool posts the same body (rule 6).
fn solve_head_reply(
    root: &std::path::Path,
    v: &Venue,
    id: &str,
    body: &Value,
) -> Result<Value, CallError> {
    let fixture = body["fixture"]
        .as_str()
        .ok_or_else(|| CallError::Bad("which fixture: `fixture`".into()))?;
    let target = parse_target(&body["target"])
        .ok_or_else(|| CallError::Bad("solve_head needs a `target`: [x, y, z] in metres".into()))?;
    let f = v
        .fixtures
        .iter()
        .find(|f| f.id == fixture)
        .ok_or_else(|| CallError::Bad(format!("no fixture {fixture:?} in this venue")))?;
    let limits = f
        .profile
        .as_deref()
        .and_then(|m| crate::fixture::get(root, m))
        .map(|p| p.limits)
        .unwrap_or_default();
    let state = v
        .solve_head(fixture, target, &limits)
        .map_err(CallError::Bad)?;
    let at = Vec3::from(f.at.unwrap_or([0.0; 3]));
    let hit = v
        .hit(at, venue::direction(state.actual.pan, state.actual.tilt))
        .map(|h| json!({ "what": h.what, "at": h.at, "distance": h.distance, "normal": h.normal }));
    Ok(json!({
        "ok": true,
        "venue": id,
        "fixture": fixture,
        "state": state,
        "pan": state.actual.pan,
        "tilt": state.actual.tilt,
        "hit": hit,
    }))
}

/// `[x, y, z]` from a JSON array, when it is exactly three finite numbers.
fn parse_target(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let mut out = [0.0f32; 3];
    for (i, x) in a.iter().enumerate() {
        let n = x.as_f64()? as f32;
        if !n.is_finite() {
            return None;
        }
        out[i] = n;
    }
    Some(out)
}

/// `[[s, t], …]` from a JSON array of pairs, when every pair is two finite
/// numbers.
fn parse_path(v: &Value) -> Option<Vec<[f32; 2]>> {
    let a = v.as_array()?;
    let mut out = Vec::with_capacity(a.len());
    for p in a {
        let pair = p.as_array()?;
        if pair.len() != 2 {
            return None;
        }
        let s = pair[0].as_f64()? as f32;
        let t = pair[1].as_f64()? as f32;
        if !s.is_finite() || !t.is_finite() {
            return None;
        }
        out.push([s, t]);
    }
    Some(out)
}

/// One first-class object form from a JSON string or a `{form, …}` object.
/// Caps keep an imported mesh from exhausting memory; every number must be
/// finite.
fn parse_object_form(v: &Value) -> Result<venue::ObjectForm, String> {
    match v {
        Value::String(word) => match word.as_str() {
            "box" => Ok(venue::ObjectForm::Box),
            "swept" => Ok(venue::ObjectForm::Swept {
                path: Vec::new(),
                profile: Vec::new(),
                caps: false,
                closed: false,
                kind: "catmull_rom".into(),
            }),
            "mesh" => Ok(venue::ObjectForm::Mesh {
                positions: Vec::new(),
                indices: Vec::new(),
                uvs: Vec::new(),
                normals: Vec::new(),
            }),
            "capsule" => Ok(venue::ObjectForm::Capsule {
                radius: 0.22,
                height: 1.8,
            }),
            other => Err(format!(
                "unknown form {other:?} (box, swept, mesh, capsule)"
            )),
        },
        Value::Object(map) => {
            let tag = map
                .get("form")
                .and_then(Value::as_str)
                .ok_or("the object needs a `form` tag")?;
            match tag {
                "box" => Ok(venue::ObjectForm::Box),
                "swept" => Ok(venue::ObjectForm::Swept {
                    path: parse_vec3_list(map.get("path"), 4096).unwrap_or_default(),
                    profile: parse_vec2_list(map.get("profile"), 1024).unwrap_or_default(),
                    caps: map.get("caps").and_then(Value::as_bool).unwrap_or(false),
                    closed: map.get("closed").and_then(Value::as_bool).unwrap_or(false),
                    kind: map
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("catmull_rom")
                        .to_string(),
                }),
                "mesh" => Ok(venue::ObjectForm::Mesh {
                    positions: parse_vec3_list(map.get("positions"), 200_000)?,
                    indices: parse_u32_list(map.get("indices"), 600_000)?.unwrap_or_default(),
                    uvs: parse_vec2_list(map.get("uvs"), 200_000).unwrap_or_default(),
                    normals: parse_vec3_list(map.get("normals"), 200_000).unwrap_or_default(),
                }),
                "capsule" => Ok(venue::ObjectForm::Capsule {
                    radius: finite_or(map.get("radius"), 0.22, "radius")?,
                    height: finite_or(map.get("height"), 1.8, "height")?,
                }),
                other => Err(format!(
                    "unknown form {other:?} (box, swept, mesh, capsule)"
                )),
            }
        }
        _ => Err("`form` is a string or an object".into()),
    }
}

fn finite_or(v: Option<&Value>, default: f32, name: &str) -> Result<f32, String> {
    match v {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            let n = v.as_f64().ok_or_else(|| format!("`{name}` is a number"))? as f32;
            if n.is_finite() {
                Ok(n)
            } else {
                Err(format!("`{name}` must be finite"))
            }
        }
    }
}

fn parse_vec3_list(v: Option<&Value>, cap: usize) -> Result<Vec<[f32; 3]>, String> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let a = v.as_array().ok_or("expected a list of [x, y, z]")?;
    if a.len() > cap {
        return Err(format!("{cap} points at most"));
    }
    a.iter()
        .map(|p| parse_target(p).ok_or_else(|| "every point is [x, y, z] and finite".into()))
        .collect()
}

fn parse_vec2_list(v: Option<&Value>, cap: usize) -> Result<Vec<[f32; 2]>, String> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let a = v.as_array().ok_or("expected a list of [x, y]")?;
    if a.len() > cap {
        return Err(format!("{cap} points at most"));
    }
    let mut out = Vec::with_capacity(a.len());
    for p in a {
        let pair = p
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or("every point is [x, y]")?;
        let x = pair[0].as_f64().ok_or("every point is [x, y]")? as f32;
        let y = pair[1].as_f64().ok_or("every point is [x, y]")? as f32;
        if !x.is_finite() || !y.is_finite() {
            return Err("every point is finite".into());
        }
        out.push([x, y]);
    }
    Ok(out)
}

fn parse_u32_list(v: Option<&Value>, cap: usize) -> Result<Option<Vec<u32>>, String> {
    let Some(v) = v else {
        return Ok(None);
    };
    let a = v.as_array().ok_or("expected a list of indices")?;
    if a.len() > cap {
        return Err(format!("{cap} indices at most"));
    }
    let mut out = Vec::with_capacity(a.len());
    for x in a {
        let n = x.as_u64().ok_or("every index is a non-negative integer")?;
        out.push(u32::try_from(n).map_err(|_| "an index is too large".to_string())?);
    }
    Ok(Some(out))
}

/// The body `stage`/`truss`/`dummy` hand to [`edit_object`]: the op forces
/// the object's `kind`, and picks a form when the caller gave none.
fn object_op_body(body: &Value, op: &str) -> Value {
    let mut body = body.clone();
    if body.get("kind").is_none() {
        body["kind"] = Value::String(op.to_string());
    }
    if body.get("form").is_none() {
        let form = match op {
            "dummy" => Some(json!({ "form": "capsule", "radius": 0.22, "height": 1.8 })),
            "stage" | "truss" if body.get("path").is_some() => {
                let size = body.get("size").and_then(vec3).unwrap_or([1.0, 1.0, 1.0]);
                let (w, h) = (size[2] / 2.0, size[1]);
                Some(json!({
                    "form": "swept",
                    "path": body["path"],
                    "profile": [[-w, 0.0], [w, 0.0], [w, h], [-w, h]],
                    "caps": body["caps"].as_bool().unwrap_or(false),
                    "closed": body["closed"].as_bool().unwrap_or(false),
                }))
            }
            "stage" | "truss" => Some(json!({ "form": "box" })),
            _ => None,
        };
        if let Some(form) = form {
            body["form"] = form;
        }
    }
    body
}

/// `op: "boolean"`: CSG between two objects' world-space meshes. The result
/// is written as an imported mesh (`at = [0, 0, 0]`, `yaw = 0`) because the
/// meshes are already in world space.
fn boolean_op(v: &mut Venue, root: &std::path::Path, body: &Value) -> Result<Value, CallError> {
    let a = body["a"]
        .as_str()
        .ok_or_else(|| CallError::Bad("boolean needs `a`".into()))?;
    let b = body["b"]
        .as_str()
        .ok_or_else(|| CallError::Bad("boolean needs `b`".into()))?;
    let result = body["result"].as_str().unwrap_or(a).to_string();
    let operation = match body["operation"].as_str().unwrap_or("union") {
        "union" => metal_geom::csg::BooleanOp::Union,
        "difference" => metal_geom::csg::BooleanOp::Difference,
        "intersect" | "intersection" => metal_geom::csg::BooleanOp::Intersect,
        other => {
            return Err(CallError::Bad(format!(
                "operation {other:?} (union, difference, intersect)"
            )))
        }
    };
    let mesh_of = |id: &str| {
        v.objects
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.mesh())
            .ok_or_else(|| CallError::Bad(format!("no object {id:?} in this venue")))
    };
    let ma = mesh_of(a)?;
    let mb = mesh_of(b)?;
    let csg = metal_geom::csg::boolean(operation, &ma, &mb)
        .map_err(|e| CallError::Bad(format!("CSG failed: {e:?}")))?;
    let triangles = csg.indices.len() / 3;
    let form = venue::ObjectForm::Mesh {
        positions: csg.positions,
        indices: csg.indices,
        uvs: csg.uvs,
        normals: csg.normals,
    };
    if let Some(o) = v.objects.iter_mut().find(|o| o.id == result) {
        o.form = form;
        o.at = [0.0; 3];
        o.yaw = 0.0;
        o.crown = None;
        if let Some(name) = body["name"].as_str() {
            o.name = name.to_string();
        }
    } else {
        v.objects.push(venue::Object {
            id: result.clone(),
            name: body["name"].as_str().unwrap_or(&result).to_string(),
            kind: "mesh".into(),
            at: [0.0; 3],
            size: [1.0, 1.0, 1.0],
            yaw: 0.0,
            crown: None,
            occludes: true,
            emits: 0.0,
            faces: Vec::new(),
            description: String::new(),
            form,
            parent: None,
            attribution: String::new(),
            source: String::new(),
        });
    }
    v.save(root).map_err(CallError::Bad)?;
    let object = v.objects.iter().find(|o| o.id == result).cloned();
    Ok(json!({
        "ok": true, "id": v.venue.id, "object": object, "triangles": triangles,
    }))
}

struct RenderCamera {
    eye: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    tan_fov: f32,
    aspect: f32,
}

fn camera_from_shot(c: &venue::Camera) -> RenderCamera {
    let forward = Vec3::from(venue::direction(c.yaw, c.pitch)).normalize_or_zero();
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let right = if right.length_squared() < 1.0e-9 {
        Vec3::X
    } else {
        right
    };
    RenderCamera {
        eye: Vec3::from(c.eye),
        forward,
        right,
        up: right.cross(forward).normalize_or_zero(),
        tan_fov: (c.fov.to_radians() * 0.5).tan().max(1.0e-4),
        aspect: if c.aspect > 0.0 { c.aspect } else { 16.0 / 9.0 },
    }
}

/// A tiny CPU rasteriser: the same lambert+z-buffer the viewport's ASCII
/// "view" answer uses. No camera or geometry still returns a blank grid.
fn view_op(v: &Venue, body: &Value) -> Result<Value, CallError> {
    let width = body["width"].as_u64().unwrap_or(64).clamp(16, 160) as usize;
    let height = body["height"].as_u64().unwrap_or(36).clamp(16, 120) as usize;
    let shot = body["shot"]
        .as_str()
        .and_then(|id| v.shots.iter().find(|s| s.id == id))
        .or_else(|| v.shots.first());
    let (camera, label) = match shot {
        Some(s) => (camera_from_shot(&s.camera), s.id.clone()),
        None => {
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            let mut any = false;
            for o in &v.objects {
                if let Some((a, b)) = o.bbox() {
                    lo = lo.min(Vec3::from(a));
                    hi = hi.max(Vec3::from(b));
                    any = true;
                }
            }
            for s in &v.surfaces {
                let (a, b) = s.bbox();
                lo = lo.min(Vec3::from(a));
                hi = hi.max(Vec3::from(b));
                any = true;
            }
            let centre = if any {
                (lo + hi) * 0.5
            } else {
                Vec3::new(0.0, 1.0, 0.0)
            };
            let d = if any {
                (hi - lo).length().max(4.0)
            } else {
                8.0
            };
            let eye = centre + Vec3::new(d, d * 0.6, d);
            let forward = (centre - eye).normalize_or_zero();
            let right = forward.cross(Vec3::Y).normalize_or_zero();
            let right = if right.length_squared() < 1.0e-9 {
                Vec3::X
            } else {
                right
            };
            (
                RenderCamera {
                    eye,
                    forward,
                    right,
                    up: right.cross(forward).normalize_or_zero(),
                    tan_fov: 45.0f32.to_radians().tan(),
                    aspect: width as f32 / height as f32,
                },
                "orbit".to_string(),
            )
        }
    };

    let mut zbuf = vec![f32::INFINITY; width * height];
    let mut shade = vec![0.0f32; width * height];
    let light = Vec3::new(0.4, 0.8, 0.45).normalize();
    let project = |p: [f32; 3]| -> Option<(f32, f32, f32)> {
        let d = Vec3::from(p) - camera.eye;
        let z = d.dot(camera.forward);
        if z <= 1.0e-4 {
            return None;
        }
        let x = d.dot(camera.right) / z;
        let y = d.dot(camera.up) / z;
        let sx = (x / (camera.tan_fov * camera.aspect) + 1.0) * 0.5 * width as f32;
        let sy = (1.0 - y / camera.tan_fov) * 0.5 * height as f32;
        Some((sx, sy, z))
    };

    let mut meshes: Vec<metal_geom::Mesh> = v.objects.iter().map(|o| o.mesh()).collect();
    meshes.extend(v.surfaces.iter().map(|s| s.tessellate(8, 4)));
    let mut tris = Vec::new();
    for m in &meshes {
        tris.extend(m.triangles());
    }
    let mut draw = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
        let Some((ax, ay, az)) = project(a) else {
            return;
        };
        let Some((bx, by, bz)) = project(b) else {
            return;
        };
        let Some((cx, cy, cz)) = project(c) else {
            return;
        };
        let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if area.abs() < 1.0e-9 {
            return;
        }
        let normal = (Vec3::from(b) - Vec3::from(a))
            .cross(Vec3::from(c) - Vec3::from(a))
            .normalize_or_zero();
        let lambert = normal.dot(light).abs().clamp(0.0, 1.0);
        let brightness = 0.22 + 0.78 * lambert;
        let minx = ax.min(bx).min(cx).floor().max(0.0) as usize;
        let maxx = (ax.max(bx).max(cx).ceil() as isize).clamp(0, width as isize - 1) as usize;
        let miny = ay.min(by).min(cy).floor().max(0.0) as usize;
        let maxy = (ay.max(by).max(cy).ceil() as isize).clamp(0, height as isize - 1) as usize;
        for py in miny..=maxy {
            for px in minx..=maxx {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let w0 = ((bx - fx) * (cy - fy) - (by - fy) * (cx - fx)) / area;
                let w1 = ((cx - fx) * (ay - fy) - (cy - fy) * (ax - fx)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1.0e-4 || w1 < -1.0e-4 || w2 < -1.0e-4 {
                    continue;
                }
                let z = w0 * az + w1 * bz + w2 * cz;
                let i = py * width + px;
                if z < zbuf[i] {
                    zbuf[i] = z;
                    shade[i] = brightness;
                }
            }
        }
    };
    for t in &tris {
        draw(t.a, t.b, t.c);
    }

    let ramp = " .:-=+*#%@";
    let rows: Vec<String> = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| {
                    let i = y * width + x;
                    if !zbuf[i].is_finite() {
                        ' '
                    } else {
                        let n = ((shade[i].clamp(0.05, 1.0) * (ramp.chars().count() - 1) as f32)
                            .round() as usize)
                            .min(ramp.chars().count() - 1);
                        ramp.chars().nth(n).unwrap_or(' ')
                    }
                })
                .collect()
        })
        .collect();

    if body["format"].as_str() == Some("png") {
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, width as u32, height as u32);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc
                .write_header()
                .map_err(|e| CallError::Bad(format!("png: {e}")))?;
            let pixels: Vec<u8> = (0..width * height)
                .map(|i| {
                    if !zbuf[i].is_finite() {
                        0
                    } else {
                        (shade[i].clamp(0.0, 1.0) * 255.0).round() as u8
                    }
                })
                .collect();
            writer
                .write_image_data(&pixels)
                .map_err(|e| CallError::Bad(format!("png: {e}")))?;
        }
        return Ok(json!({
            "ok": true, "id": v.venue.id, "width": width, "height": height,
            "format": "png",
            "png": format!("data:image/png;base64,{}", base64_encode(&bytes)),
            "camera": label,
        }));
    }

    Ok(json!({
        "ok": true, "id": v.venue.id, "width": width, "height": height,
        "ramp": ramp, "rows": rows,
        "legend": {
            "ramp": ramp, "shading": "lambert", "depth": "nearer is brighter",
            "camera": label,
        },
    }))
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// `op: "fixture"`: add, edit or remove one fixture of `v`, through
/// [`venue::Fixture`]'s own fields.
///
/// `kind` is validated against [`FixtureKind::from_word`] and a new fixture
/// must name one; `address` is clamped to a universe's 512 slots
/// ([`venue::clamp_address`]); a `profile` that names no file under
/// `data/fixtures/` is refused before anything is written, so a typo cannot
/// ride as a fixture nothing drives. A field left out of the body is left
/// alone (so an edit can touch one key), and a `null` clears an optional one.
/// The caller saves with `Venue::save`; this never writes itself.
fn edit_fixture(v: &mut Venue, root: &std::path::Path, body: &Value) -> Result<Value, String> {
    // Remove first: `id` names the fixture, so a delete needs no other field.
    if body["delete"].as_bool().unwrap_or(false) {
        let id = body["id"].as_str().ok_or("which fixture to remove: `id`")?;
        let before = v.fixtures.len();
        v.fixtures.retain(|f| f.id != id);
        if v.fixtures.len() == before {
            return Err(format!("no fixture {id:?} in this venue"));
        }
        return Ok(json!({ "deleted": id }));
    }

    let wanted = body["id"].as_str().map(str::to_string);
    let existing = wanted
        .as_deref()
        .and_then(|id| v.fixtures.iter().position(|f| f.id == id));
    // `kind` is validated whenever it is given; a new fixture must name one.
    let kind = match body["kind"].as_str() {
        Some(word) => venue::FixtureKind::from_word(word)
            .ok_or_else(|| format!("unknown fixture kind {word:?}"))?,
        None => match existing {
            Some(i) => v.fixtures[i].kind,
            None => return Err("a new fixture needs a `kind`".into()),
        },
    };

    // A profile has to name a file under `data/fixtures/`.
    if let Some(p) = body
        .get("profile")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
    {
        if crate::fixture::get(root, p).is_none() {
            return Err(format!(
                "unknown fixture profile {p:?}: no data/fixtures/{p}.toml"
            ));
        }
    }

    // D4: parse (and validate) a matrix body before anything is written, so a
    // malformed grid leaves the venue untouched. `None` leaves it alone; the
    // inner `None` clears it.
    let matrix: Option<Option<venue::Matrix>> = match body.get("matrix") {
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(matrix_from_body(v)?)),
        None => None,
    };

    let id = match existing {
        Some(i) => v.fixtures[i].id.clone(),
        None => {
            let id = v.unique_fixture_id(kind, wanted.clone());
            v.fixtures
                .push(venue::Fixture::new(id.clone(), id.clone(), kind));
            id
        }
    };
    let f = v
        .fixtures
        .iter_mut()
        .find(|f| f.id == id)
        .expect("the fixture is on the venue");

    f.kind = kind;
    if let Some(name) = body["name"].as_str() {
        f.name = name.to_string();
    }
    if let Some(notes) = body["notes"].as_str() {
        f.notes = notes.to_string();
    }
    match body.get("profile") {
        Some(Value::Null) => f.profile = None,
        Some(Value::String(p)) => f.profile = (!p.is_empty()).then(|| p.clone()),
        _ => {}
    }
    match body.get("group") {
        Some(Value::Null) => f.group = None,
        Some(Value::String(g)) => f.group = (!g.is_empty()).then(|| g.clone()),
        _ => {}
    }
    if let Some(w) = body["wire"].as_str() {
        f.wire = w.to_string();
    }
    match body.get("universe") {
        Some(Value::Null) => f.universe = None,
        Some(Value::Number(n)) => {
            f.universe = Some(
                n.as_u64()
                    .ok_or("`universe` is a number or null")?
                    .min(u16::MAX as u64) as u16,
            )
        }
        Some(_) => return Err("`universe` is a number or null".into()),
        None => {}
    }
    match body.get("address") {
        Some(Value::Null) => f.address = None,
        Some(Value::Number(n)) => {
            f.address = Some(venue::clamp_address(
                n.as_u64()
                    .ok_or("`address` is a number or null")?
                    .min(u16::MAX as u64) as u16,
            ))
        }
        Some(_) => return Err("`address` is a number or null".into()),
        None => {}
    }
    match body.get("pixels") {
        Some(Value::Null) => f.pixels = None,
        Some(Value::Number(n)) => {
            f.pixels = Some(n.as_u64().ok_or("`pixels` is a number or null")? as usize)
        }
        Some(_) => return Err("`pixels` is a number or null".into()),
        None => {}
    }
    if let Some(start) = body["start"].as_u64() {
        f.start = start as usize;
    }
    if let Some(order) = body["order"].as_str() {
        f.order = match order {
            "forward" => venue::PixelOrder::Forward,
            "reverse" => venue::PixelOrder::Reverse,
            other => return Err(format!("unknown order {other:?}: forward or reverse")),
        };
    }
    if let Some(drive) = body["drive"].as_str() {
        f.drive = match drive {
            "frame" => venue::Drive::Frame,
            "fixed" => venue::Drive::Fixed,
            other => return Err(format!("unknown drive {other:?}: frame or fixed")),
        };
    }
    match body.get("projector") {
        Some(Value::Null) => f.projector = None,
        Some(Value::String(p)) => f.projector = (!p.is_empty()).then(|| p.clone()),
        _ => {}
    }
    // D4: a matrix fixture's grid, scan order and channel map. `null` clears
    // it; a matrix body was validated above.
    match matrix {
        Some(m) => f.matrix = m,
        None if f.kind == venue::FixtureKind::Matrix && f.matrix.is_none() => {
            f.matrix = Some(venue::Matrix::default());
        }
        None => {}
    }

    serde_json::to_value(&*f).map_err(|e| e.to_string())
}

/// D4: a `matrix` object from a `POST /venue { op: "fixture" }` body — the
/// ShowXpress field set. Unknown corner/scan words and a grid a `dmx_matrix`
/// does not match are refused, so a typo cannot ride as a matrix nothing maps.
fn matrix_from_body(v: &Value) -> Result<venue::Matrix, String> {
    if !v.is_object() {
        return Err("`matrix` is an object".into());
    }
    let int = |key: &str| -> Result<usize, String> {
        v.get(key)
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .ok_or_else(|| format!("`matrix.{key}` is a number"))
    };
    let mut m = venue::Matrix::default();
    if v.get("x_pixels").is_some() {
        m.x_pixels = int("x_pixels")?;
    }
    if v.get("y_pixels").is_some() {
        m.y_pixels = int("y_pixels")?;
    }
    if let Some(s) = v.get("start_corner").and_then(Value::as_str) {
        m.start_corner = venue::StartCorner::from_word(s).ok_or_else(|| {
            format!("unknown start_corner {s:?}: bottom_left, bottom_right, top_left, top_right")
        })?;
    }
    if let Some(s) = v.get("orientation").and_then(Value::as_str) {
        m.orientation = venue::Orientation::from_word(s)
            .ok_or_else(|| format!("unknown orientation {s:?}: horizontal or vertical"))?;
    }
    if let Some(s) = v.get("snake").and_then(Value::as_str) {
        m.snake =
            venue::Snake::from_word(s).ok_or_else(|| format!("unknown snake {s:?}: off or on"))?;
    }
    if v.get("first_channels").is_some() {
        m.first_channels = int("first_channels")?;
    }
    if v.get("last_channels").is_some() {
        m.last_channels = int("last_channels")?;
    }
    if let Some(a) = v.get("dmx_matrix") {
        let arr = a
            .as_array()
            .ok_or("`matrix.dmx_matrix` is an array of channel offsets")?;
        m.dmx_matrix = arr
            .iter()
            .map(|x| {
                x.as_i64()
                    .map(|n| n as i16)
                    .ok_or_else(|| "a `matrix.dmx_matrix` entry is an integer".to_string())
            })
            .collect::<Result<_, _>>()?;
    }
    m.check()?;
    Ok(m)
}

/// Where an AI draft for `id` waits for accept/reject (M5.3).
fn draft_path(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    venue::dir(root).join(format!("{id}.draft.toml"))
}

fn save_draft(root: &std::path::Path, id: &str, v: &Venue) -> Result<(), String> {
    let text = toml::to_string_pretty(v).map_err(|e| e.to_string())?;
    metal_vfs::create_dir_all(venue::dir(root)).map_err(|e| e.to_string())?;
    metal_vfs::write(draft_path(root, id), text).map_err(|e| e.to_string())
}

fn load_draft(root: &std::path::Path, id: &str) -> Option<Venue> {
    let text = metal_vfs::read_to_string(draft_path(root, id)).ok()?;
    Venue::parse(&text).ok()
}

fn remove_draft(root: &std::path::Path, id: &str) {
    let _ = metal_vfs::remove_file(draft_path(root, id));
}

/// A flat diff between the current venue and a draft (M5.3): one row per
/// entry that the draft adds, removes or changes, per array. The editor
/// paints it beside the two venues.
fn diff_rows(current: &Venue, proposed: &Venue) -> Vec<Value> {
    let mut rows = Vec::new();
    let sets: [(&str, Vec<&str>, Vec<&str>); 6] = [
        (
            "surface",
            current.surfaces.iter().map(|x| x.id.as_str()).collect(),
            proposed.surfaces.iter().map(|x| x.id.as_str()).collect(),
        ),
        (
            "object",
            current.objects.iter().map(|x| x.id.as_str()).collect(),
            proposed.objects.iter().map(|x| x.id.as_str()).collect(),
        ),
        (
            "projector",
            current.projectors.iter().map(|x| x.id.as_str()).collect(),
            proposed.projectors.iter().map(|x| x.id.as_str()).collect(),
        ),
        (
            "fixture",
            current.fixtures.iter().map(|x| x.id.as_str()).collect(),
            proposed.fixtures.iter().map(|x| x.id.as_str()).collect(),
        ),
        (
            "mask",
            current.masks.iter().map(|x| x.id.as_str()).collect(),
            proposed.masks.iter().map(|x| x.id.as_str()).collect(),
        ),
        (
            "display",
            current.displays.iter().map(|x| x.id.as_str()).collect(),
            proposed.displays.iter().map(|x| x.id.as_str()).collect(),
        ),
    ];
    for (kind, before, after) in sets {
        for id in &after {
            if !before.contains(id) {
                rows.push(json!({ "kind": kind, "id": id, "change": "added" }));
            }
        }
        for id in &before {
            if !after.contains(id) {
                rows.push(json!({ "kind": kind, "id": id, "change": "removed" }));
            }
        }
        if before.len() == after.len() && before != after {
            rows.push(json!({ "kind": kind, "change": "reordered" }));
        }
    }
    rows
}

/// With a `device`, keep one projector and point it at that device.
fn retarget(v: &mut Venue, device: Option<&str>, projector: Option<&str>) -> Result<(), String> {
    let Some(device) = device else {
        if let Some(p) = projector {
            v.projectors.retain(|x| x.id == p);
        }
        return match v.projectors.is_empty() {
            true => Err("no such projector in this venue".into()),
            false => Ok(()),
        };
    };
    let at = match projector {
        Some(p) => v
            .projectors
            .iter()
            .position(|x| x.id == p)
            .ok_or_else(|| format!("no projector {p:?} in this venue"))?,
        None if v.projectors.is_empty() => return Err("this venue has no projector".into()),
        None => 0,
    };
    let mut chosen = v.projectors.swap_remove(at);
    chosen.output = device.to_string();
    v.projectors = vec![chosen];
    Ok(())
}

/// A `[x, y, z]` from a JSON value, or `None` for anything else.
fn vec3(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([
        a[0].as_f64()? as f32,
        a[1].as_f64()? as f32,
        a[2].as_f64()? as f32,
    ])
}

/// P3 `op: "surface"`: create, edit or delete one surface of the venue.
///
/// `points` (four `[x, y, z]` corners, `(0,0) (1,0) (1,1) (0,1)` in that
/// order) is the honest way to draw one — it sets `origin`/`u`/`v`. The three
/// vectors are accepted directly too, so a caller that already has the venue's
/// own vocabulary can edit a surface without converting.
fn edit_surface(v: &mut Venue, body: &Value) -> Result<(), String> {
    let id = body["surface"].as_str().ok_or("which surface: `surface`")?;
    if body["delete"].as_bool().unwrap_or(false) {
        v.surfaces.retain(|s| s.id != id);
        for p in &mut v.projectors {
            p.pins.remove(id);
        }
        return Ok(());
    }
    let geometry = match body.get("points").and_then(Value::as_array) {
        Some(pts) if pts.len() == 4 => {
            let c: Vec<[f32; 3]> = pts
                .iter()
                .map(|p| vec3(p).ok_or("a surface point is [x, y, z]"))
                .collect::<Result<_, _>>()?;
            let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
            Some((c[0], sub(c[1], c[0]), sub(c[3], c[0])))
        }
        Some(_) => return Err("`points` is four [x, y, z] corners".into()),
        None => None,
    };
    if let Some(s) = v.surfaces.iter_mut().find(|s| s.id == id) {
        if let Some(name) = body["name"].as_str() {
            s.name = name.to_string();
        }
        if let Some(material) = body["material"].as_str() {
            s.material = material.to_string();
        }
        if let Some((o, u, w)) = geometry {
            s.origin = o;
            s.u = u;
            s.v = w;
        } else {
            if let Some(o) = vec3(&body["origin"]) {
                s.origin = o;
            }
            if let Some(u) = vec3(&body["u"]) {
                s.u = u;
            }
            if let Some(w) = vec3(&body["v"]) {
                s.v = w;
            }
        }
        return Ok(());
    }
    let (origin, u, vv) = match geometry {
        Some(g) => g,
        None => (
            vec3(&body["origin"]).ok_or("a new surface needs `points` or `origin`")?,
            vec3(&body["u"]).ok_or("a new surface needs `points` or `u`")?,
            vec3(&body["v"]).ok_or("a new surface needs `points` or `v`")?,
        ),
    };
    v.surfaces.push(venue::Surface {
        id: id.to_string(),
        name: body["name"].as_str().unwrap_or(id).to_string(),
        origin,
        u,
        v: vv,
        material: body["material"].as_str().unwrap_or_default().to_string(),
        form: Default::default(),
        sectors: Default::default(),
    });
    Ok(())
}

/// P-F: per surface with ≥ 2 pinned projectors, which projectors are on it,
/// each one's `blend`, and the overlap polygons in the surface's `(s, t)` —
/// what the venue editor hatches.
pub(crate) fn blend_report(v: &Venue) -> Value {
    let mut out = serde_json::Map::new();
    for s in &v.surfaces {
        let on: Vec<&venue::Projector> = v
            .projectors
            .iter()
            .filter(|p| p.pins.contains_key(&s.id))
            .collect();
        if on.len() < 2 {
            continue;
        }
        let overlaps: Vec<Value> = metal_perform::edge_blend::overlaps(v, &s.id)
            .into_iter()
            .map(|(a, b, points)| json!({ "a": a, "b": b, "points": points }))
            .collect();
        out.insert(
            s.id.clone(),
            json!({
                "projectors": on.iter().map(|p| json!({ "id": p.id, "output": p.output, "blend": p.blend })).collect::<Vec<_>>(),
                "enabled": on.iter().all(|p| p.blend.enabled),
                "overlaps": overlaps,
            }),
        );
    }
    Value::Object(out)
}

/// P-F `op: "blend"`: the named `projector`, or every projector pinned to
/// `surface`, gets the fields the body carries (`enabled`, `gamma`, `curve`,
/// `black_lift`); the rest keep their value. Answers the ids it changed.
fn set_blend(v: &mut Venue, body: &Value) -> Result<Vec<String>, String> {
    let pid = body["projector"].as_str();
    let sid = body["surface"].as_str();
    if pid.is_none() && sid.is_none() {
        return Err("which projector or surface: `projector` / `surface`".into());
    }
    let curve = match body["curve"].as_str() {
        Some(w) => Some(
            metal_perform::edge_blend::Curve::from_word(w)
                .ok_or_else(|| format!("curve {w:?}: linear, smoothstep or cosine"))?,
        ),
        None => None,
    };
    let gamma = body["gamma"].as_f64().map(|g| g as f32);
    if gamma.is_some_and(|g| !(0.5..=4.0).contains(&g)) {
        return Err("gamma is 0.5..4.0".into());
    }
    let lift = body["black_lift"].as_f64().map(|g| g as f32);
    if lift.is_some_and(|l| !(0.0..=0.5).contains(&l)) {
        return Err("black_lift is 0..0.5".into());
    }
    let mut changed = Vec::new();
    for p in v.projectors.iter_mut() {
        let hit = match (pid, sid) {
            (Some(pid), _) => p.id == pid,
            (None, Some(sid)) => p.pins.contains_key(sid),
            _ => false,
        };
        if !hit {
            continue;
        }
        if let Some(on) = body["enabled"].as_bool() {
            p.blend.enabled = on;
        }
        if let Some(g) = gamma {
            p.blend.gamma = g;
        }
        if let Some(c) = curve {
            p.blend.curve = c;
        }
        if let Some(l) = lift {
            p.blend.black_lift = l;
        }
        changed.push(p.id.clone());
    }
    if changed.is_empty() {
        return Err(match (pid, sid) {
            (Some(pid), _) => format!("no projector {pid:?} in this venue"),
            (None, Some(sid)) => format!("no projector is pinned to surface {sid:?}"),
            _ => unreachable!(),
        });
    }
    Ok(changed)
}

/// P3 `op: "pins"`: one projector's four device-space corners for one surface
/// — the hand calibration `calibrar` opens the shared contour editor on. The
/// four points are `(0,0) (1,0) (1,1) (0,1)` of the surface, in that order.
fn set_pins(v: &mut Venue, body: &Value) -> Result<(), String> {
    let pid = body["projector"]
        .as_str()
        .ok_or("which projector: `projector`")?;
    let sid = body["surface"].as_str().ok_or("which surface: `surface`")?;
    let p = v
        .projectors
        .iter_mut()
        .find(|p| p.id == pid)
        .ok_or_else(|| format!("no projector {pid:?} in this venue"))?;
    if body["clear"].as_bool().unwrap_or(false) {
        p.pins.remove(sid);
        return Ok(());
    }
    let pins: [[f32; 2]; 4] = serde_json::from_value(body["pins"].clone())
        .map_err(|_| "`pins` is four [x, y] corners".to_string())?;
    p.pins.insert(sid.to_string(), pins);
    Ok(())
}

/// `add_outputs: true`: every output the venue drives and this install does
/// not have yet is added the way `POST /outputs {op:"add"}` adds one — a
/// display at its own resolution, a projector at 1920x1080, a peer's output
/// shown on its `node`. Without it a missing output is the error `apply`
/// explains.
fn add_outputs(ctx: &mut Ctx, v: &Venue) {
    let state = &mut *ctx.state;
    let wanted = v
        .projectors
        .iter()
        .map(|p| (p.output.clone(), p.name.clone(), [1920, 1080], None))
        .chain(v.displays.iter().map(|d| {
            let node = (d.link == venue::Link::Peer)
                .then(|| d.node.clone())
                .flatten();
            (d.output.clone(), d.name.clone(), d.resolution, node)
        }));
    for (id, label, [w, h], node) in wanted {
        if state.outputs.find(&id).is_some() || id == crate::PREVIEW {
            continue;
        }
        let mut out = crate::output::Output::new(&id, &label, w, h);
        out.node = node;
        state.outputs.list.push(out);
        state
            .sources
            .entry(id.clone())
            .or_insert_with(crate::layers::Device::default);
        crate::ensure_device_graph_at(&crate::output::data_root(), &id, &label);
    }
}

/// Each `[[composition]]` as one source routed to every display it spans,
/// each display showing its window (`Layer::span`). The layer is found again
/// by its name, `<venue>.<composition>`, so projecting twice updates it
/// instead of stacking a second one. Goes through `apply_layers_op`, the
/// function `POST /layers` runs, and rewrites every touched device's graph
/// the way that route does.
fn compose(ctx: &mut Ctx, v: &Venue) -> Result<Vec<Value>, CallError> {
    let mut out = Vec::new();
    let ids = ctx.paint.ids();
    let models = ctx.paint.model_count();
    let limits = ctx.state.limits;
    for c in &v.compositions {
        let spans = v.spans(c).map_err(CallError::Bad)?;
        let Some(model) = ids.iter().position(|id| *id == c.model) else {
            return Err(CallError::Bad(format!(
                "composition {} draws {:?}, which is not a model this build has",
                c.id, c.model
            )));
        };
        let name = format!("{}.{}", v.venue.id, c.id);
        let devices: Vec<String> = spans.iter().map(|(o, _)| o.clone()).collect();
        if let Some(missing) = devices.iter().find(|d| ctx.state.outputs.find(d).is_none()) {
            return Err(CallError::Bad(format!(
                "composition {} spans output {missing:?}, which does not exist: pass `add_outputs: true`",
                c.id
            )));
        }
        let first = devices[0].clone();
        let lid = match ctx.state.layers.iter().find(|l| l.name == name) {
            Some(l) => l.id.clone(),
            None => crate::apply_layers_op(
                ctx.state,
                models,
                limits,
                &json!({ "op": "add", "device": first, "model": model, "name": name }),
            )
            .map_err(CallError::Bad)?
            .map(|l| l.id)
            .ok_or_else(|| CallError::Internal("the layer was not created".into()))?,
        };
        for d in &devices {
            crate::apply_layers_op(
                ctx.state,
                models,
                limits,
                &json!({ "op": "route", "device": d, "layer": lid }),
            )
            .map_err(CallError::Bad)?;
        }
        let span: serde_json::Map<String, Value> = spans
            .iter()
            .map(|(o, s)| (o.clone(), json!({ "x": s.x, "y": s.y, "w": s.w, "h": s.h })))
            .collect();
        let mut set =
            json!({ "op": "set", "device": first, "layer": lid, "model": model, "span": span });
        if let Some(shader) = &c.shader {
            set["shader"] = json!(shader);
        }
        crate::apply_layers_op(ctx.state, models, limits, &set).map_err(CallError::Bad)?;
        crate::call::outputs::sync_graphs(ctx, devices.clone())?;
        out.push(json!({ "composition": c.id, "layer": lid, "model": c.model, "span": span }));
    }
    if !out.is_empty() {
        if let Err(e) = ctx
            .state
            .outputs
            .save(&ctx.state.sources, &ctx.state.layers)
        {
            eprintln!("  outputs not saved: {e}");
        }
    }
    Ok(out)
}

/// Each projector's frame-driven fixtures, written into its device's graph as
/// the `out.*` nodes the render loop drives every frame
/// (`metal_perform::wire::drive_device`): see [`write_fixtures`]. A projector
/// whose device has no graph yet and no fixture to write is left alone.
fn fixtures(root: &std::path::Path, v: &Venue) -> Result<Vec<Value>, CallError> {
    let mut out = Vec::new();
    for p in &v.projectors {
        let gid = crate::device_graph_id(&p.output);
        let mut g = match crate::load_graph_at(root, &gid) {
            Ok(g) => g,
            Err(_) if !v.fixtures.iter().any(|f| samples(v, f, p)) => continue,
            Err(_) => {
                crate::ensure_device_graph_at(root, &p.output, &p.name);
                crate::load_graph_at(root, &gid).map_err(CallError::Internal)?
            }
        };
        let written = write_fixtures(&mut g, v, p, root);
        if written.changed {
            crate::save_graph_at(root, &g).map_err(CallError::Internal)?;
        }
        if !written.nodes.is_empty() || !written.notes.is_empty() {
            out.push(json!({ "device": p.output, "graph": gid, "nodes": written.nodes, "notes": written.notes }));
        }
    }
    Ok(out)
}

/// Whether fixture `f` takes its colour from projector `p`'s frame: it names
/// `p`, or names none and `p` is the venue's first projector (the same
/// default `Venue::fixture_points` takes).
fn samples(v: &Venue, f: &venue::Fixture, p: &venue::Projector) -> bool {
    f.drive == venue::Drive::Frame
        && match &f.projector {
            Some(id) => *id == p.id,
            None => v.projectors.first().is_some_and(|first| first.id == p.id),
        }
}

/// What [`write_fixtures`] did to one device graph.
struct Written {
    nodes: Vec<Value>,
    notes: Vec<String>,
    changed: bool,
}

/// The venue fixtures that sample projector `p`, as `out.*` nodes of `g`
/// (the graph of the device `p` is):
///
/// - washes and spots on `wire = "dmx"` or `"sacn"` are the `heads` of one
///   `out.sacn` node, one universe (`metal_perform::wire::SampledSacn`): each
///   head its profile, its address and the point of the frame its beam lands
///   on (`Venue::fixture_points`). A new head is addressed after every head
///   already there, from its profile's own address on, so four washes of one
///   profile sit one after another instead of on top of each other.
/// - each LED strip on `wire = "usb"` is an `out.serial` node (Adalight):
///   one `sample` point per LED, `pixels` their count.
///
/// Re-projecting rewrites what the venue's geometry decides (where each
/// light samples, how many LEDs) and keeps what the operator set on the node
/// (universe, unicast, a head's address, port, baud, colour order). A node
/// this venue wrote (config `venue`) that it no longer needs is removed;
/// nodes without that tag are never touched.
fn write_fixtures(
    g: &mut metal_graph::Graph,
    v: &Venue,
    p: &venue::Projector,
    root: &std::path::Path,
) -> Written {
    let vid = v.venue.id.as_str();
    let tag = |f: &venue::Fixture| format!("{vid}.{}", f.id);
    let mut notes = Vec::new();
    // (tag, profile, sample) per DMX light; (tag, samples) per USB strip.
    let mut lights: Vec<(String, String, Option<[f32; 2]>)> = Vec::new();
    let mut strips: Vec<(String, Vec<Option<[f32; 2]>>)> = Vec::new();
    for f in v.fixtures.iter().filter(|f| samples(v, f, p)) {
        let points = match v.fixture_points(f) {
            Ok(points) => points,
            Err(e) => {
                notes.push(format!("fixture {}: {e}", f.id));
                continue;
            }
        };
        let kind = match f.kind {
            venue::FixtureKind::Wash => "wash",
            venue::FixtureKind::Spot => "spot",
            venue::FixtureKind::LedStrip => "led_strip",
            venue::FixtureKind::Moving => "moving",
            venue::FixtureKind::Backlight => "backlight",
            venue::FixtureKind::Panel => "panel",
            venue::FixtureKind::Continuous => "continuous",
            venue::FixtureKind::LedBar => "led_bar",
            venue::FixtureKind::Camera => "camera",
            venue::FixtureKind::Matrix => "matrix",
        };
        match (kind, f.wire.as_str()) {
            ("wash" | "spot" | "moving" | "backlight" | "panel" | "continuous" | "led_bar", "dmx" | "sacn") => match &f.profile {
                Some(profile) => lights.push((tag(f), profile.clone(), points.first().copied().flatten())),
                None => notes.push(format!("fixture {}: a light on DMX needs a `profile` under data/fixtures/", f.id)),
            },
            ("camera", _) => notes.push(format!(
                "fixture {}: a camera is a sensor, not an emitter; nothing is sent to it",
                f.id
            )),
            ("matrix", "dmx" | "sacn") => notes.push(format!(
                "fixture {}: a matrix is driven cell by cell by a `venue.fixtures` node on its device graph; this projector writes no `out.sacn` head for it",
                f.id
            )),
            ("led_strip", "usb") => strips.push((tag(f), points)),
            (_, "bluetooth") => notes.push(format!(
                "fixture {}: a {kind} on a Bluetooth profile is authored here but its `out.bluetooth` node is not written by the venue projector yet",
                f.id
            )),
            (_, wire) => notes.push(format!(
                "fixture {}: nothing drives a {kind} on wire {wire:?} yet (lights go out on sACN from `dmx`/`sacn`, LED strips on `usb`, Bluetooth lights on `bluetooth`)",
                f.id
            )),
        }
    }

    let tagged = |n: &metal_graph::Node| {
        n.config
            .get("venue")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let config_of = |n: &metal_graph::Node| -> serde_json::Map<String, Value> {
        serde_json::from_str::<Value>(&n.config.to_json_str())
            .ok()
            .and_then(|c| c.as_object().cloned())
            .unwrap_or_default()
    };
    let before = g.to_json();
    let anchor = g
        .nodes
        .iter()
        .find(|n| n.kind == "output.device")
        .map(|n| n.pos)
        .unwrap_or([800.0, 0.0]);
    let mut nodes = Vec::new();

    // The lights: one `out.sacn` node for the venue on this device.
    let sacn = g
        .nodes
        .iter()
        .find(|n| n.kind == "out.sacn" && tagged(n).as_deref() == Some(vid))
        .map(|n| n.id.clone());
    match (sacn, lights.is_empty()) {
        (Some(id), true) => g.remove_node(&id),
        (None, true) => {}
        (id, false) => {
            let id = id.unwrap_or_else(|| {
                let id = g.add_node("out", "out.sacn", [anchor[0], anchor[1] + 140.0]);
                if let Some(n) = g.node_mut(&id) {
                    n.label = format!("{vid} lights (sACN)");
                }
                id
            });
            let node = g.node(&id).expect("found or just added");
            let mut cfg = config_of(node);
            let kept: Vec<metal_perform::wire::SampledHead> =
                metal_perform::wire::SampledSacn::from_node(node).heads;
            let footprint = |model: &str| -> Option<(u16, u16)> {
                let profile = crate::fixture::get(root, model)?;
                let last = profile
                    .channels
                    .iter()
                    .flat_map(|c| c.coarse.into_iter().chain(c.fine))
                    .max()?;
                Some((profile.address, last + 1))
            };
            // Addresses the operator (or an earlier projection) already gave.
            let old = |t: &str| kept.iter().find(|h| h.id == t).and_then(|h| h.address);
            // D3: route through the one allocator (`venue_packets` uses the
            // same), so a projector's `out.sacn` node and a `venue.fixtures`
            // node agree on where each head lands.
            let mut placements = Vec::new();
            let mut usable: Vec<(&String, &String, &Option<[f32; 2]>)> = Vec::new();
            for (t, profile, sample) in &lights {
                match footprint(profile) {
                    Some((start, span)) => {
                        placements.push(metal_perform::dmx_bus::HeadPlacement {
                            id: t.clone(),
                            universe: 0,
                            address: start,
                            span,
                        });
                        usable.push((t, profile, sample));
                    }
                    None => notes.push(format!(
                        "fixture {t}: profile {profile:?} is not under data/fixtures/"
                    )),
                }
            }
            let addresses = metal_perform::dmx_bus::allocate_addresses(&placements, old);
            let mut heads = Vec::new();
            for ((t, profile, sample), address) in usable.into_iter().zip(addresses) {
                let span = footprint(profile).map(|(_, s)| s).unwrap_or(1);
                if address as usize + span as usize - 1 > 512 {
                    notes.push(format!("fixture {t}: address {address} + {span} slots runs past slot 512 of the universe"));
                }
                heads.push(metal_perform::wire::SampledHead {
                    id: t.clone(),
                    fixture: profile.clone(),
                    address: Some(address),
                    sample: *sample,
                });
            }
            cfg.insert("venue".into(), json!(vid));
            cfg.insert("heads".into(), json!(heads));
            cfg.entry("universe").or_insert(json!(1));
            let universe = cfg["universe"].clone();
            let unicast = cfg.get("unicast").cloned().unwrap_or(Value::Null);
            if let Some(n) = g.node_mut(&id) {
                n.config = metal_graph::Config::from_json_str(&Value::Object(cfg).to_string())
                    .expect("a JSON object always parses");
            }
            nodes.push(json!({ "node": id, "kind": "out.sacn", "universe": universe, "unicast": unicast, "heads": heads }));
            if unicast.is_null() {
                notes.push(format!(
                    "{id} sends universe {universe} by multicast; point it at your sACN node with POST /graph/node {{op: \"config\", graph: \"{}\", node: \"{id}\", config: {{unicast: \"<ip>\", universe: <n>}}}}",
                    g.id
                ));
            }
        }
    }

    // The strips: one `out.serial` node each.
    let stale: Vec<String> = g
        .nodes
        .iter()
        .filter(|n| n.kind == "out.serial")
        .filter(|n| {
            tagged(n).is_some_and(|t| {
                t.starts_with(&format!("{vid}.")) && !strips.iter().any(|(s, _)| *s == t)
            })
        })
        .map(|n| n.id.clone())
        .collect();
    for id in stale {
        g.remove_node(&id);
    }
    for (i, (t, points)) in strips.iter().enumerate() {
        let id = match g
            .nodes
            .iter()
            .find(|n| n.kind == "out.serial" && tagged(n).as_deref() == Some(t.as_str()))
        {
            Some(n) => n.id.clone(),
            None => {
                let id = g.add_node(
                    "out",
                    "out.serial",
                    [anchor[0], anchor[1] + 250.0 + 110.0 * i as f32],
                );
                if let Some(n) = g.node_mut(&id) {
                    n.label = format!("{t} (USB)");
                }
                id
            }
        };
        let mut cfg = config_of(g.node(&id).expect("found or just added"));
        cfg.insert("venue".into(), json!(t));
        cfg.insert("pixels".into(), json!(points.len()));
        cfg.insert("sample".into(), json!(points));
        let port = cfg
            .get("port")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(n) = g.node_mut(&id) {
            n.config = metal_graph::Config::from_json_str(&Value::Object(cfg).to_string())
                .expect("a JSON object always parses");
        }
        nodes.push(json!({ "node": id, "kind": "out.serial", "fixture": t, "pixels": points.len(), "port": port }));
        if port.is_empty() {
            notes.push(format!(
                "{id} ({t}) has no serial port yet, so it stays dark: POST /graph/node {{op: \"config\", graph: \"{}\", node: \"{id}\", config: {{port: \"/dev/ttyUSB0\"}}}}",
                g.id
            ));
        }
    }

    let changed = g.to_json() != before;
    Written {
        nodes,
        notes,
        changed,
    }
}

fn project(ctx: &mut Ctx, v: &Venue) -> Result<Vec<Value>, CallError> {
    let state = &mut *ctx.state;
    let devices = apply(&mut state.outputs, v).map_err(CallError::Bad)?;
    if let Err(e) = state.outputs.save(&state.sources, &state.layers) {
        eprintln!("  outputs not saved: {e}");
    }
    Ok(devices)
}

/// The write itself, on an `Outputs` in memory: each projector's masks onto
/// its output under the `<venue>.` prefix. One reply entry per device, with
/// the legend (id, name, colour, what should play) and the reasons the test
/// projection could look wrong.
fn apply(outputs: &mut Outputs, v: &Venue) -> Result<Vec<Value>, String> {
    let prefix = format!("{}.", v.venue.id);
    let mut devices = Vec::new();
    // Projectors first, then displays: `(output, what drives it, aspect)`.
    let driven = v
        .projectors
        .iter()
        .map(|p| {
            (
                p.output.clone(),
                format!("projector {}", p.id),
                p.aspect,
                Some(p.id.clone()),
            )
        })
        .chain(v.displays.iter().map(|d| {
            let aspect = d.resolution[0] as f32 / d.resolution[1].max(1) as f32;
            (d.output.clone(), format!("display {}", d.id), aspect, None)
        }));
    for (output, what, want_aspect, projector) in driven {
        let Some(out) = outputs.find_mut(&output) else {
            return Err(format!(
                "{what} drives output {output:?}, which does not exist: add it with POST /outputs {{op: \"add\"}}, pass `device`, or pass `add_outputs: true`"
            ));
        };
        let masks = v.project(&output)?;
        out.masks.retain(|m| !m.id.starts_with(&prefix));
        let mut legend = Vec::new();
        // The test projection's colours, ready to post as `POST /overlay
        // {op:"solid", masks}`: the venue's `rgb` for each mask, or the same
        // palette colour `look_venue` draws it in when it has none.
        let mut solid = serde_json::Map::new();
        for mut m in masks {
            let at = v.masks.iter().position(|x| x.id == m.id);
            let source = at.map(|i| &v.masks[i]);
            let colour = at.map(|i| v.masks[i].rgb.unwrap_or(crate::output::mask_color(i)));
            legend.push(json!({
                "id": format!("{prefix}{}", m.id),
                "name": m.name,
                "rgb": source.and_then(|s| s.rgb),
                "content": source.map(|s| s.content.as_str()).unwrap_or_default(),
            }));
            m.id = format!("{prefix}{}", m.id);
            m.parent = m.parent.map(|x| format!("{prefix}{x}"));
            if let Some(c) = colour {
                solid.insert(m.id.clone(), json!(c));
            }
            out.masks.push(m);
        }
        let aspect = out.width as f32 / out.height.max(1) as f32;
        let mut notes = Vec::new();
        if (aspect - want_aspect).abs() / want_aspect > 0.02 {
            notes.push(format!(
                "the output is {}x{} ({aspect:.3}), the {what} is {want_aspect:.3}: contours stretch",
                out.width, out.height
            ));
        }
        // A device with a `mask` node cuts per layer and skips the
        // whole-frame pass, so a solid overlay covers the whole frame: its
        // masks show their own colours, everything else the solid's `rgb`.
        let cut = !crate::graph_has_mask_node(&crate::output::data_root(), &output);
        if !cut {
            notes.push("this device's graph has a mask node: the whole-frame cut is skipped, so outside its masks the test projection shows the solid's own rgb (black when only mask colours are posted)".to_string());
        }
        devices.push(json!({
            "device": output,
            "projector": projector,
            "driven_by": what,
            "masks": legend,
            "solid": solid,
            "cut": cut,
            "notes": notes,
        }));
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{Mask, Output};

    /// The screen op reconciles a physical aspect with its pixels, on the
    /// side the body names.
    #[test]
    fn screen_aspect_correction_obeys_the_lock() {
        let mut w = 2.0;
        let mut h = 1.0;
        let mut px = [1920, 1080];
        assert!(correct_aspect(&mut w, &mut h, &mut px, Some("physical")));
        assert_eq!(px, [1920, 960]);
        assert_eq!((w, h), (2.0, 1.0));

        let mut w = 2.0;
        let mut h = 1.0;
        let mut px = [1920, 1080];
        assert!(correct_aspect(&mut w, &mut h, &mut px, Some("pixels")));
        assert_eq!(px, [1920, 1080]);
        assert!((h - 1.125).abs() < 1e-6);
        assert!((w / h - 16.0 / 9.0).abs() < 1e-6);

        let mut w = 16.0;
        let mut h = 9.0;
        let mut px = [1920, 1080];
        assert!(!correct_aspect(&mut w, &mut h, &mut px, None));
    }

    /// A screen's plane starts at its top-left corner and its `v` runs down,
    /// before and after a yaw about `y`.
    #[test]
    fn screen_geometry_places_the_glass_top_left() {
        let (origin, u, v) = screen_geometry([0.0, 1.0, 0.0], None, 2.0, 1.0);
        assert_eq!(origin, [-1.0, 1.5, 0.0]);
        assert_eq!(u, [2.0, 0.0, 0.0]);
        assert_eq!(v, [0.0, -1.0, 0.0]);
        let (origin, u, v) = screen_geometry([0.0, 1.0, 0.0], Some(90.0), 2.0, 1.0);
        for (got, want) in origin.iter().zip([0.0, 1.5, -1.0]) {
            assert!((got - want).abs() < 1e-6, "{origin:?}");
        }
        for (got, want) in u.iter().zip([0.0, 0.0, 2.0]) {
            assert!((got - want).abs() < 1e-6, "{u:?}");
        }
        for (got, want) in v.iter().zip([0.0, -1.0, 0.0]) {
            assert!((got - want).abs() < 1e-6, "{v:?}");
        }
    }

    /// `surface.form` parses a plane or a swept body, with defaults and
    /// refusal for too few path/profile points.
    #[test]
    fn surface_form_bodies_parse() {
        assert_eq!(
            surface_form_from_body(&json!({ "form": "plane" })).unwrap(),
            venue::SurfaceForm::Plane
        );
        let form = surface_form_from_body(&json!({
            "form": "swept",
            "path": [[0, 0, 0], [1, 0, 0]],
            "profile": [[0, 0], [1, 0], [0, 1]],
            "caps": true,
        }))
        .unwrap();
        match form {
            venue::SurfaceForm::Swept {
                path,
                profile,
                caps,
                closed,
                kind,
            } => {
                assert_eq!(path.len(), 2);
                assert_eq!(profile.len(), 3);
                assert!(caps);
                assert!(!closed);
                assert_eq!(kind, "catmull_rom");
            }
            other => panic!("expected a sweep, got {other:?}"),
        }
        assert!(surface_form_from_body(&json!({
            "form": "swept",
            "path": [[0, 0, 0]],
            "profile": [[0, 0], [1, 0], [0, 1]],
        }))
        .is_err());
        assert!(surface_form_from_body(&json!({
            "form": "swept",
            "path": [[0, 0, 0], [1, 0, 0]],
            "profile": [[0, 0], [1, 0]],
        }))
        .is_err());
    }

    /// `surface.sectors` upserts one surface's grid: every field is optional
    /// and a body that names a field updates just that one.
    #[test]
    fn surface_sectors_upserts_one_grid() {
        let mut v: Venue = toml::from_str(
            "[venue]\nid = \"t\"\nname = \"t\"\nsetting = \"indoor\"\n\
             [[surface]]\nid = \"w\"\nname = \"W\"\norigin = [0,0,0]\nu = [1,0,0]\nv = [0,1,0]\n",
        )
        .unwrap();
        let s = set_surface_sectors(
            &mut v,
            &json!({ "surface": "w", "source": "points", "ratio": 2.0,
                    "points": [[0.5, 0.0], [0.5, 1.0]], "magnet": true }),
        )
        .unwrap();
        assert_eq!(s.sectors.source, venue::SectorSource::Points);
        assert_eq!(s.sectors.ratio, 2.0);
        assert!(s.sectors.magnet);
        // A second body naming only `ratio` keeps the drawn points.
        let s = set_surface_sectors(&mut v, &json!({ "surface": "w", "ratio": 3.0 })).unwrap();
        assert_eq!(s.sectors.points.len(), 2, "the points survive a ratio edit");
        assert_eq!(s.sectors.ratio, 3.0);
        // Refusals: an unknown source, an odd point count, a point outside.
        assert!(set_surface_sectors(&mut v, &json!({ "surface": "w", "source": "x" })).is_err());
        assert!(set_surface_sectors(
            &mut v,
            &json!({ "surface": "w", "points": [[0.1, 0.1]] })
        )
        .is_err());
        assert!(set_surface_sectors(
            &mut v,
            &json!({ "surface": "w", "points": [[2.0, 0.0], [0.0, 1.0]] })
        )
        .is_err());
        assert!(set_surface_sectors(&mut v, &json!({ "surface": "nope" })).is_err());
    }

    /// A move to `at` applies exactly `at - current`.
    #[test]
    fn move_delta_is_the_offset_to_the_target() {
        let current = [1.0, 2.0, 3.0];
        let at = [4.0, 6.0, 3.0];
        let d = move_delta(current, at);
        assert_eq!(d, [3.0, 4.0, 0.0]);
        assert_eq!(
            [current[0] + d[0], current[1] + d[1], current[2] + d[2]],
            at
        );
    }

    fn bar() -> Venue {
        venue::load(&crate::output::data_root(), "bar").expect("the bar example")
    }

    /// D5: the `solve_head` op body inverts the aim — a point on the back
    /// wall solves, the live state and the beam's landing point come back,
    /// and a body naming no fixture or no target is refused by name.
    #[test]
    fn solve_head_op_points_a_head_and_reports_the_hit() {
        let v = bar();
        let wall = v.walls().into_iter().find(|w| w.id == "back-wall").unwrap();
        let target = wall.at(0.5, 0.4).to_array();
        let root = crate::output::data_root();
        let out = solve_head_reply(
            &root,
            &v,
            "bar",
            &json!({ "op": "solve_head", "fixture": "wash-1", "target": target }),
        )
        .expect("solve_head");
        assert_eq!(out["ok"], true);
        assert_eq!(out["fixture"], "wash-1");
        let hit = &out["hit"];
        assert_eq!(hit["what"], "back-wall");
        let at = hit["at"].as_array().unwrap();
        for (k, want) in target.iter().enumerate() {
            assert!(
                (at[k].as_f64().unwrap() as f32 - want).abs() < 1e-3,
                "{at:?} vs {target:?}"
            );
        }
        assert!(out["state"]["commanded"]["pan"].is_number());
        assert!(out["pan"].is_number() && out["tilt"].is_number());
        assert!(solve_head_reply(&root, &v, "bar", &json!({ "target": target })).is_err());
        assert!(solve_head_reply(&root, &v, "bar", &json!({ "fixture": "wash-1" })).is_err());
        assert!(solve_head_reply(
            &root,
            &v,
            "bar",
            &json!({ "fixture": "nope", "target": target })
        )
        .is_err());
    }

    #[test]
    fn projecting_a_venue_replaces_its_own_masks_and_keeps_the_operators() {
        let mut outputs = Outputs {
            list: vec![Output::new("screen-1", "Screen", 1920, 1080)],
        };
        outputs.list[0].masks.push(Mask {
            id: "mask-1".into(),
            name: "hand-drawn".into(),
            points: vec![[0.0, 0.0], [0.1, 0.0], [0.1, 0.1]],
            parent: None,
            alpha: 1.0,
            compose: None,
            stroke: None,
        });
        let mut v = bar();
        retarget(&mut v, Some("screen-1"), None).unwrap();
        let first = apply(&mut outputs, &v).unwrap();
        let ids = |o: &Outputs| {
            o.list[0]
                .masks
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&outputs),
            [
                "mask-1",
                "bar.panel-left",
                "bar.panel-right",
                "bar.counter-band"
            ]
        );
        assert_eq!(first[0]["masks"][0]["rgb"], json!([255, 120, 40]));
        // Each mask's own test colour, keyed the way `POST /overlay
        // {op:"solid", masks}` takes it.
        assert_eq!(
            first[0]["solid"],
            json!({ "bar.panel-left": [255, 120, 40], "bar.panel-right": [40, 140, 255],
                    "bar.counter-band": [255, 255, 255] })
        );
        assert!(
            first[0]["notes"].as_array().unwrap().is_empty(),
            "{first:?}"
        );
        // Twice is the same as once: the venue's own masks are replaced.
        apply(&mut outputs, &v).unwrap();
        assert_eq!(ids(&outputs).len(), 4);
        // And the contours are the ones the venue projects, in device space.
        let want = v.project("screen-1").unwrap();
        assert_eq!(outputs.list[0].masks[1].points, want[0].points);
    }

    /// A display's masks land on its own output point for point, next to
    /// the projector's projected ones — one `apply`, both kinds of output.
    #[test]
    fn a_screen_gets_its_glass_masks_unprojected() {
        let v = venue::load(&crate::output::data_root(), "living-room").expect("example");
        let mut outputs = Outputs {
            list: vec![
                Output::new("proyector-sala", "halo", 1920, 1080),
                Output::new("tv", "TV", 3840, 2160),
            ],
        };
        let devices = apply(&mut outputs, &v).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[1]["driven_by"], "display tv");
        let tv = outputs.find("tv").unwrap();
        assert_eq!(tv.masks.len(), 1);
        assert_eq!(tv.masks[0].id, "living-room.tv-picture");
        assert_eq!(
            tv.masks[0].points,
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        );
        let halo = outputs.find("proyector-sala").unwrap();
        assert_eq!(
            halo.masks.len(),
            3,
            "the three halo panels, never the TV's own mask"
        );
        assert!(
            devices
                .iter()
                .all(|d| d["notes"].as_array().unwrap().is_empty()),
            "{devices:?}"
        );
    }

    /// P-F: `op: "blend"` on a surface reaches every projector pinned to it,
    /// and `GET /venue`'s `blend` then reports it with the overlap.
    #[test]
    fn blend_on_a_surface_sets_every_pinned_projector() {
        let mut v = bar();
        let sid = v.surfaces[0].id.clone();
        let mut second = v.projectors[0].clone();
        second.id = "p2".into();
        second.output = "p2".into();
        v.projectors[0].pins.insert(
            sid.clone(),
            [[0.0, 0.0], [1.6, 0.0], [1.6, 1.0], [0.0, 1.0]],
        );
        second.pins.insert(
            sid.clone(),
            [[-0.6, 0.0], [1.0, 0.0], [1.0, 1.0], [-0.6, 1.0]],
        );
        v.projectors.push(second);
        assert!(set_blend(&mut v, &json!({ "surface": "nowhere", "enabled": true })).is_err());
        assert!(set_blend(&mut v, &json!({ "surface": sid, "curve": "wobbly" })).is_err());
        let changed = set_blend(
            &mut v,
            &json!({ "surface": sid, "enabled": true, "black_lift": 0.05 }),
        )
        .unwrap();
        assert_eq!(changed.len(), 2);
        assert!(v
            .projectors
            .iter()
            .all(|p| p.blend.enabled && p.blend.black_lift == 0.05 && p.blend.gamma == 2.2));
        let report = blend_report(&v);
        assert_eq!(report[&sid]["enabled"], json!(true));
        assert_eq!(report[&sid]["overlaps"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_projector_aimed_at_a_missing_output_says_how_to_fix_it() {
        let mut outputs = Outputs { list: vec![] };
        let err = apply(&mut outputs, &bar()).unwrap_err();
        assert!(
            err.contains("proyector-bar") && err.contains("device"),
            "{err}"
        );
    }

    #[test]
    fn retargeting_keeps_one_projector_and_renames_its_output() {
        let mut v = bar();
        assert!(retarget(&mut v, Some("x"), Some("nope")).is_err());
        retarget(&mut v, Some("x"), Some("proj-1")).unwrap();
        assert_eq!(v.projectors.len(), 1);
        assert_eq!(v.projectors[0].output, "x");
    }

    fn device_graph(output: &str) -> metal_graph::Graph {
        let mut g = metal_graph::Graph::new(
            crate::device_graph_id(output),
            output,
            metal_graph::Domain::Live,
        );
        g.add_node("out", "output.device", [800.0, 0.0]);
        g
    }

    fn node_config(g: &metal_graph::Graph, kind: &str) -> Value {
        let n = g
            .nodes
            .iter()
            .find(|n| n.kind == kind)
            .unwrap_or_else(|| panic!("no {kind}"));
        serde_json::from_str(&n.config.to_json_str()).unwrap()
    }

    fn set_config(g: &mut metal_graph::Graph, kind: &str, key: &str, value: Value) {
        let n = g.nodes.iter_mut().find(|n| n.kind == kind).unwrap();
        let mut cfg: Value = serde_json::from_str(&n.config.to_json_str()).unwrap();
        cfg[key] = value;
        n.config = metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
    }

    /// The bar's four washes (`wire = "dmx"`) become the heads of one
    /// `out.sacn` node — one universe, laid out one after another from the
    /// profile's own address — each sampling where its beam lands, and its
    /// USB strip one `out.serial` node with a point per LED.
    #[test]
    fn projecting_writes_the_venue_fixtures_into_the_projector_graph() {
        let v = bar();
        let root = crate::output::data_root();
        let mut g = device_graph("proyector-bar");
        let written = write_fixtures(&mut g, &v, &v.projectors[0], &root);
        assert!(written.changed);
        assert_eq!(
            g.nodes.len(),
            3,
            "output.device, one out.sacn, one out.serial"
        );

        let sacn = node_config(&g, "out.sacn");
        assert_eq!(sacn["venue"], "bar");
        assert_eq!(sacn["universe"], 1);
        let heads = sacn["heads"].as_array().unwrap();
        let ids: Vec<&str> = heads.iter().map(|h| h["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            ["bar.wash-1", "bar.wash-2", "bar.wash-3", "bar.wash-4"]
        );
        let addresses: Vec<u64> = heads
            .iter()
            .map(|h| h["address"].as_u64().unwrap())
            .collect();
        assert_eq!(
            addresses,
            [20, 24, 28, 32],
            "example-wash starts at 20 and takes four slots"
        );
        for (h, f) in heads.iter().zip(v.fixtures.iter()) {
            let want = v.fixture_points(f).unwrap()[0];
            assert_eq!(
                h["sample"],
                json!(want),
                "{} samples where its beam lands",
                f.id
            );
            assert!(want.is_some(), "{} lands in the frame", f.id);
        }

        let strip = node_config(&g, "out.serial");
        let points = v
            .fixture_points(v.fixtures.iter().find(|f| f.id == "strip-1").unwrap())
            .unwrap();
        assert_eq!(strip["venue"], "bar.strip-1");
        assert_eq!(strip["pixels"], json!(points.len()));
        let cfg = metal_perform::wire::SerialConfig::from_node(
            g.nodes.iter().find(|n| n.kind == "out.serial").unwrap(),
        );
        assert_eq!(cfg.sample, points);
        assert!(
            strip.get("port").is_none(),
            "only whoever holds the cable knows the port"
        );
        assert!(
            written.notes.iter().any(|n| n.contains("multicast"))
                && written.notes.iter().any(|n| n.contains("serial port")),
            "{:?}",
            written.notes
        );
    }

    /// D3: the projector's `write_fixtures` and a `venue.fixtures` node's
    /// `venue_heads` route through the one allocator, so they agree on the
    /// address each head lands at (they can disagree on the universe: the
    /// node carries one, the profile carries its own — both are 1-based).
    #[test]
    fn dmx_bus_allocator_agrees_between_venue_packets_and_write_fixtures() {
        let v = bar();
        let root = crate::output::data_root();
        let mut g = device_graph("proyector-bar");
        write_fixtures(&mut g, &v, &v.projectors[0], &root);
        let sacn = node_config(&g, "out.sacn");
        let write_addresses: Vec<u64> = sacn["heads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["address"].as_u64().unwrap())
            .collect();

        let venue_addresses: Vec<u64> = metal_perform::wire::venue_heads(&v, None, &root)
            .iter()
            .map(|(_, h)| h.address.unwrap() as u64)
            .collect();

        assert_eq!(
            (venue_addresses, write_addresses),
            (vec![20, 24, 28, 32], vec![20, 24, 28, 32]),
            "one allocator: a venue.fixtures node and a projector agree"
        );
    }

    /// Projecting again rewrites what the geometry decides and keeps what the
    /// operator set on the nodes; a venue that no longer has fixtures takes
    /// its own nodes away and leaves everybody else's.
    #[test]
    fn reprojecting_keeps_the_operators_wire_settings() {
        let v = bar();
        let root = crate::output::data_root();
        let mut g = device_graph("proyector-bar");
        write_fixtures(&mut g, &v, &v.projectors[0], &root);
        assert!(
            !write_fixtures(&mut g, &v, &v.projectors[0], &root).changed,
            "twice is once"
        );

        set_config(&mut g, "out.sacn", "unicast", json!("10.0.0.9"));
        set_config(&mut g, "out.sacn", "universe", json!(4));
        let mut sacn = node_config(&g, "out.sacn");
        sacn["heads"][1]["address"] = json!(101);
        set_config(&mut g, "out.sacn", "heads", sacn["heads"].clone());
        set_config(&mut g, "out.serial", "port", json!("/dev/ttyACM0"));
        set_config(&mut g, "out.serial", "baud", json!(500000));
        let mine = g.add_node("out", "out.sacn", [0.0, 0.0]);

        write_fixtures(&mut g, &v, &v.projectors[0], &root);
        let sacn = node_config(&g, "out.sacn");
        assert_eq!(
            (sacn["unicast"].clone(), sacn["universe"].clone()),
            (json!("10.0.0.9"), json!(4))
        );
        let addresses: Vec<u64> = sacn["heads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["address"].as_u64().unwrap())
            .collect();
        assert_eq!(
            addresses,
            [20, 101, 28, 32],
            "every address already given is kept, the edited one too"
        );
        let strip = node_config(&g, "out.serial");
        assert_eq!(
            (strip["port"].clone(), strip["baud"].clone()),
            (json!("/dev/ttyACM0"), json!(500000))
        );

        let mut bare = v.clone();
        bare.fixtures.clear();
        write_fixtures(&mut g, &bare, &bare.projectors[0], &root);
        let kinds: Vec<&str> = g.nodes.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["output.device", "out.sacn"],
            "the venue's nodes are gone"
        );
        assert!(
            g.node(&mine).is_some(),
            "the operator's own out.sacn is untouched"
        );
    }

    /// The route's half: the projector's device graph is created when it is
    /// missing and the nodes land in the graph file the render loop reads.
    #[test]
    fn the_project_op_saves_the_fixture_nodes_in_the_device_graph() {
        let root = crate::output::data_root();
        let mut v = bar();
        let device = format!("venue-fixtures-{}", std::process::id());
        retarget(&mut v, Some(&device), None).unwrap();
        let summary = fixtures(&root, &v).expect("written");
        assert_eq!(summary[0]["device"], json!(device));
        assert_eq!(summary[0]["nodes"].as_array().unwrap().len(), 2);
        let g = crate::load_graph_at(&root, &crate::device_graph_id(&device)).expect("saved");
        assert!(g
            .nodes
            .iter()
            .any(|n| n.kind == "out.sacn" && metal_perform::wire::frame_driven(n)));
        assert!(g.nodes.iter().any(|n| n.kind == "out.serial"));
        let _ = metal_vfs::remove_file(crate::graph_path_at(
            &root,
            &crate::device_graph_id(&device),
        ));
    }

    /// The whole chain, venue to wire: the bar projected on a device, its
    /// test colours armed per mask (`solid`), one frame rendered — and the
    /// four washes, which the venue hangs so they pool on the two panels,
    /// go out on sACN in the colour of the panel each one lands on: 1 and 2
    /// amber, 3 and 4 blue, through `data/fixtures/example-wash.toml`.
    #[test]
    fn a_projected_venue_lights_each_wash_with_the_colour_of_its_panel() {
        let root = crate::output::data_root();
        let mut paint = crate::paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = crate::State::new(paint.as_ref());
        state.paused_at = Some(3.0);
        let device = format!("venue-wire-{}", std::process::id());
        state
            .outputs
            .list
            .push(Output::new(&device, "probe", 320, 180));
        let mut layer = crate::layers::Layer::new("layer-venue", "venue", 0);
        layer.outputs = vec![device.clone()];
        state.layers = vec![layer];
        state.sources.insert(
            device.clone(),
            crate::layers::Device {
                mode: crate::layers::DeviceMode::Own,
                ..Default::default()
            },
        );

        let mut v = bar();
        retarget(&mut v, Some(&device), None).unwrap();
        let devices = apply(&mut state.outputs, &v).unwrap();
        let solid: Vec<(String, [u8; 3])> = devices[0]["solid"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, c)| (id.clone(), serde_json::from_value(c.clone()).unwrap()))
            .collect();

        let recv = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        recv.set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .unwrap();
        let port = recv.local_addr().unwrap().port();
        crate::ensure_device_graph_at(&root, &device, "probe");
        let gid = crate::device_graph_id(&device);
        let mut g = crate::load_graph_at(&root, &gid).unwrap();
        write_fixtures(&mut g, &v, &v.projectors[0], &root);
        set_config(
            &mut g,
            "out.sacn",
            "unicast",
            json!(format!("127.0.0.1:{port}")),
        );
        crate::save_graph_at(&root, &g).unwrap();

        let t = state.time();
        state
            .overlays
            .start_solid_masks(device.clone(), [0, 0, 0], solid, t, 5.0);
        // The wire reads the frame through the sink's non-blocking ring, a frame
        // or two late: render until the first copy lands and the universe goes out.
        let mut buf = [0u8; 700];
        recv.set_nonblocking(true).unwrap();
        let mut sent = false;
        for _ in 0..300 {
            crate::render_device(paint.as_mut(), &mut sim, &mut state, &device, 320, 180);
            std::thread::sleep(std::time::Duration::from_millis(10));
            if recv.recv_from(&mut buf).is_ok() {
                sent = true;
                break;
            }
        }
        assert!(sent, "the lights' universe");
        let dmx = &buf[126..126 + 512];

        let wash = crate::fixture::get(&root, "example-wash").expect("the example profile");
        let heads = metal_perform::wire::SampledSacn::from_node(
            g.nodes.iter().find(|n| n.kind == "out.sacn").unwrap(),
        )
        .heads;
        let amber = [255.0 / 255.0, 120.0 / 255.0, 40.0 / 255.0];
        let blue = [40.0 / 255.0, 140.0 / 255.0, 255.0 / 255.0];
        for (head, colour) in heads.iter().zip([amber, amber, blue, blue]) {
            for (address, byte) in
                wash.frame(&head.head(), &metal_perform::wire::frame_signal(colour))
            {
                assert_eq!(
                    dmx[address as usize - 1],
                    byte,
                    "{} at address {address}",
                    head.id
                );
            }
        }
        assert_eq!(state.wire_errors, 0);
        let _ = metal_vfs::remove_file(crate::graph_path_at(&root, &gid));
    }

    // -- D1: `op: "fixture"`, the write the Settings page and `set_fixture`
    //    share. Each test builds its venue in a tempdir (rule 8) and writes
    //    only there.

    /// A minimal venue to hold fixtures, built in memory.
    fn fixture_venue() -> Venue {
        Venue::parse("[venue]\nid = \"test\"\nname = \"Test\"\nsetting = \"indoor\"\n")
            .expect("a minimal venue parses")
    }

    /// A fresh temp root for one test, so two tests never share a
    /// `data/venues/`.
    fn fixture_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("fw-fixture-{}-{name}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&root);
        root
    }

    /// D1: adding a fixture through the op writes it onto the venue, and
    /// `Venue::save`/`venue::load` (the one venue path) bring it back whole.
    #[test]
    fn adding_a_fixture_persists_and_reloads() {
        let root = fixture_root("add");
        let mut v = fixture_venue();
        let body = json!({
            "op": "fixture", "venue": "test", "kind": "wash", "name": "frente",
            "group": "front-wash", "wire": "dmx", "universe": 3, "address": 40,
        });
        let added = edit_fixture(&mut v, &root, &body).expect("adding a fixture");
        let id = added["id"]
            .as_str()
            .expect("the reply names the fixture")
            .to_string();
        assert_eq!(added["kind"], json!("wash"));
        v.save(&root).expect("save the venue");

        let back = venue::load(&root, "test").expect("load it back");
        let got = back
            .fixtures
            .iter()
            .find(|f| f.id == id)
            .expect("the fixture survived the reload");
        assert_eq!(got.name, "frente");
        assert_eq!(got.group.as_deref(), Some("front-wash"));
        assert_eq!(got.wire, "dmx");
        assert_eq!(got.universe, Some(3));
        assert_eq!(got.address, Some(40));
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// F13: the row the Discover section's "add as fixture" posts — an
    /// Art-Net port-address as the universe, wire `artnet` — writes onto the
    /// venue and reloads back whole. Built in a scratch venues dir (rule 8:
    /// the test never touches the real `data/`); the body is the one
    /// `settings::discover_fixture_body` produces.
    #[test]
    fn a_discovered_artnet_fixture_writes_the_venue_row() {
        let root = fixture_root("discover");
        let mut v = fixture_venue();
        let body = json!({
            "op": "fixture", "venue": "test", "kind": "wash", "name": "node",
            "wire": "artnet", "universe": 0x0120,
        });
        let added = edit_fixture(&mut v, &root, &body).expect("accepting a discovered row");
        let id = added["id"]
            .as_str()
            .expect("the reply names the fixture")
            .to_string();
        v.save(&root).expect("save the venue");

        let back = venue::load(&root, "test").expect("load it back");
        let got = back
            .fixtures
            .iter()
            .find(|f| f.id == id)
            .expect("the fixture survived");
        assert_eq!(got.name, "node");
        assert_eq!(got.wire, "artnet");
        assert_eq!(got.universe, Some(0x0120));
        assert_eq!(
            back.fixtures.len(),
            1,
            "accepting writes the one row and nothing else"
        );
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D1: an edit clamps `universe` to a `u16` and `address` into a
    /// universe's 512 slots, and refuses an unknown `order`/`drive` — never
    /// silently keeping the bad value.
    #[test]
    fn editing_a_fixture_clamps_and_validates_its_mapping() {
        let root = fixture_root("clamp");
        let mut v = fixture_venue();
        v.fixtures.push(venue::Fixture::new(
            "w".into(),
            "w".into(),
            venue::FixtureKind::Wash,
        ));
        let body = json!({ "op": "fixture", "venue": "test", "id": "w", "kind": "wash", "universe": 70000, "address": 9999 });
        let edited = edit_fixture(&mut v, &root, &body).expect("a clamped edit");
        assert_eq!(edited["universe"], json!(65535), "a universe is a u16");
        assert_eq!(edited["address"], json!(512), "a universe has 512 slots");
        assert_eq!(edited["id"], json!("w"), "editing never renames the id");

        for bad in [
            json!({ "op": "fixture", "venue": "test", "id": "w", "kind": "wash", "order": "sideways" }),
            json!({ "op": "fixture", "venue": "test", "id": "w", "kind": "wash", "drive": "sometimes" }),
        ] {
            assert!(edit_fixture(&mut v, &root, &bad).is_err(), "{bad}");
        }
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D1: a fixture that names no existing kind — or a profile with no file
    /// under `data/fixtures/` — is refused before anything is written, so a
    /// typo never rides as a fixture nothing drives.
    #[test]
    fn an_unknown_fixture_kind_or_profile_is_refused() {
        let root = fixture_root("unknown");
        let mut v = fixture_venue();
        let kind = json!({ "op": "fixture", "venue": "test", "kind": "laser" });
        let err = edit_fixture(&mut v, &root, &kind).unwrap_err();
        assert!(err.contains("laser") && err.contains("kind"), "{err}");

        let profile =
            json!({ "op": "fixture", "venue": "test", "kind": "wash", "profile": "nope" });
        let err = edit_fixture(&mut v, &root, &profile).unwrap_err();
        assert!(err.contains("nope") && err.contains("profile"), "{err}");
        assert!(
            v.fixtures.is_empty(),
            "nothing is written on a refused body"
        );
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D1: the op round-trips through the venue file — add with a real
    /// profile, edit its group, remove it — each step saved and read back,
    /// the same `POST /venue {op:"fixture"}` the settings page posts.
    #[test]
    fn the_fixture_op_round_trips_through_save_and_load() {
        let root = fixture_root("roundtrip");
        // A real profile under the tempdir's own `fixtures/` (rule 8: never
        // the repo's `data/`).
        let real = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/fixtures/example-wash.toml");
        metal_vfs::create_dir_all(crate::fixture::dir(&root)).unwrap();
        metal_vfs::write(
            crate::fixture::dir(&root).join("example-wash.toml"),
            metal_vfs::read_to_string(&real).unwrap(),
        )
        .unwrap();

        let mut v = fixture_venue();
        let add = json!({ "op": "fixture", "venue": "test", "kind": "wash", "profile": "example-wash", "address": 20 });
        let added = edit_fixture(&mut v, &root, &add).expect("a known profile is accepted");
        let id = added["id"].as_str().unwrap().to_string();
        assert_eq!(added["profile"], json!("example-wash"));
        v.save(&root).unwrap();

        let edit =
            json!({ "op": "fixture", "venue": "test", "id": id, "kind": "wash", "group": "front" });
        edit_fixture(&mut v, &root, &edit).expect("editing the group");
        v.save(&root).unwrap();
        let back = venue::load(&root, "test").unwrap();
        assert_eq!(
            back.fixtures
                .iter()
                .find(|f| f.id == id)
                .unwrap()
                .group
                .as_deref(),
            Some("front")
        );

        let del = json!({ "op": "fixture", "venue": "test", "id": id, "delete": true });
        let reply = edit_fixture(&mut v, &root, &del).expect("removing the fixture");
        assert_eq!(reply["deleted"], json!(id));
        v.save(&root).unwrap();
        let back = venue::load(&root, "test").unwrap();
        assert!(
            back.fixtures.iter().all(|f| f.id != id),
            "the fixture is gone"
        );
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D4: adding a matrix fixture through the op writes its grid, scan order
    /// and channel map, saves them and reads them back whole — the same
    /// `POST /venue {op:"fixture"}` the Settings page posts.
    #[test]
    fn adding_a_matrix_fixture_persists_and_reloads() {
        let root = fixture_root("matrix-add");
        let mut v = fixture_venue();
        let body = json!({
            "op": "fixture", "venue": "test", "kind": "matrix", "name": "media",
            "wire": "sacn", "universe": 2, "address": 300,
            "matrix": {
                "x_pixels": 4, "y_pixels": 3,
                "start_corner": "top_right", "orientation": "vertical", "snake": "on",
                "first_channels": 1, "last_channels": 2,
            },
        });
        let added = edit_fixture(&mut v, &root, &body).expect("adding a matrix");
        assert_eq!(added["kind"], json!("matrix"));
        assert_eq!(added["matrix"]["x_pixels"], json!(4));
        assert_eq!(added["matrix"]["snake"], json!("on"));
        v.save(&root).expect("save the venue");

        let back = venue::load(&root, "test").expect("load it back");
        let got = back
            .fixtures
            .iter()
            .find(|f| f.kind == venue::FixtureKind::Matrix)
            .expect("the matrix survived the reload");
        let m = got.matrix.as_ref().expect("its matrix is on the fixture");
        assert_eq!((m.x_pixels, m.y_pixels), (4, 3));
        assert_eq!(m.start_corner, venue::StartCorner::TopRight);
        assert_eq!(m.orientation, venue::Orientation::Vertical);
        assert_eq!(m.snake, venue::Snake::On);
        assert_eq!(got.universe, Some(2));
        assert_eq!(got.address, Some(300));
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D4: a matrix body with an unknown scan word, or a `dmx_matrix` that
    /// does not match its grid, is refused before anything is written.
    #[test]
    fn a_malformed_matrix_is_refused() {
        let root = fixture_root("matrix-bad");
        let mut v = fixture_venue();
        for bad in [
            json!({ "op": "fixture", "venue": "test", "kind": "matrix",
                    "matrix": { "x_pixels": 2, "y_pixels": 2, "start_corner": "middle" } }),
            json!({ "op": "fixture", "venue": "test", "kind": "matrix",
                    "matrix": { "x_pixels": 2, "y_pixels": 2, "dmx_matrix": [1, 2, 3] } }),
        ] {
            assert!(edit_fixture(&mut v, &root, &bad).is_err(), "{bad}");
        }
        assert!(
            v.fixtures.is_empty(),
            "nothing is written on a refused body"
        );
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D2: `POST /fixtures {op:"import-gdtf"}` reads a real `.gdtf` (a zip
    /// built here), writes `data/fixtures/<model>.toml` under a tempdir
    /// (rule 8) and answers `{ok, model, channels}`. A body naming neither
    /// `path` nor `bytes` is a 400, not a panic.
    #[test]
    fn gdtf_import_writes_a_profile_under_the_data_dir() {
        let root = std::env::temp_dir().join(format!("fw-dash-gdtf-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&root);
        metal_vfs::create_dir_all(&root).unwrap();
        let gdtf = root.join("probe.gdtf");
        metal_vfs::write(&gdtf, &stored_zip("description.xml", GDTF_XML.as_bytes())).unwrap();

        let reply = fixtures_import(
            &root,
            &json!({ "op": "import-gdtf", "path": gdtf.to_str().unwrap() }),
        )
        .expect("import");
        assert_eq!(reply["ok"], json!(true));
        assert_eq!(reply["model"], json!("probewash"));
        assert_eq!(reply["channels"], json!(3));
        // On disk as the model `GET /fixtures` would list, and it loads back.
        assert!(crate::fixture::get(&root, "probewash").is_some());
        assert!(crate::fixture::dir(&root).join("probewash.toml").exists());

        // Neither a path nor bytes is refused.
        assert!(fixtures_import(&root, &json!({ "op": "import-gdtf" })).is_err());
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// A minimal GDTF mode: 16-bit pan, a dimmer and one colour byte.
    const GDTF_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<GDTF DataVersion="1.2">
 <FixtureType Name="Probe Wash" ShortName="ProbeWash" Manufacturer="MetalVJ">
  <DMXModes>
   <DMXMode Name="Standard" Geometry="Body">
    <DMXChannels>
     <DMXChannel Offset="1" Geometry="Yoke">
      <LogicalChannel Attribute="Pan">
       <ChannelFunction Attribute="Pan" DMXFrom="0/2" DMXTo="65535/2" PhysicalFrom="-270" PhysicalTo="270"/>
      </LogicalChannel>
     </DMXChannel>
     <DMXChannel Offset="3" Geometry="Body">
      <LogicalChannel Attribute="Dimmer">
       <ChannelFunction Attribute="Dimmer" DMXFrom="0/1" DMXTo="255/1"/>
      </LogicalChannel>
     </DMXChannel>
     <DMXChannel Offset="4" Geometry="Body">
      <LogicalChannel Attribute="ColorAdd_R">
       <ChannelFunction Attribute="ColorAdd_R" DMXFrom="0/1" DMXTo="255/1"/>
      </LogicalChannel>
     </DMXChannel>
    </DMXChannels>
   </DMXMode>
  </DMXModes>
 </FixtureType>
</GDTF>
"#;

    /// A stored-entry (method 0) zip around one file — a real archive the
    /// importer's own reader unpacks; no `zip` crate needed to build it.
    fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    let m = crc & 1 != 0;
                    crc >>= 1;
                    if m {
                        crc ^= 0xEDB8_8320;
                    }
                }
            }
            !crc
        }
        let crc = crc32(data);
        let mut out = Vec::new();
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        let cd_start = out.len() as u32;
        // Central directory.
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        let cd_size = out.len() as u32 - cd_start;
        // End of central directory.
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// F10/F11/F14: `GET /venue/fixture-packet` reads the **last packet** the
    /// node put on the bus — its own shadow, never a recomputation. A node
    /// whose venue row is gone is an orphan, a bypassed node is bypassed, and a
    /// live node with nothing on the bus is silent.
    #[test]
    fn the_fixture_packet_route_reads_the_last_packet_off_the_bus() {
        use metal_perform::dmx_bus::{DmxBus, SourceMode};
        let root = crate::output::data_root();
        let v = bar();
        let row = v
            .fixtures
            .iter()
            .find(|f| f.id == "wash-1")
            .expect("bar has wash-1");
        let profile = mkfixture(row.profile.as_deref().expect("wash-1 has a profile"));

        let mut node = metal_graph::Node::new("f-0", metal_graph::VENUE_FIXTURE_KIND, [0.0, 0.0]);
        node.config = metal_graph::Config::from_json_str(
            &json!({ "venue": "bar", "fixture": "wash-1" }).to_string(),
        )
        .unwrap();
        let prefix = "venue/device-bar/f-0/";

        // Nothing written yet: the node is live but silent.
        let quiet = fixture_packet_json(&root, &DmxBus::new(), &node, prefix);
        assert_eq!(quiet["state"], json!("silent"));
        assert_eq!(quiet["name"], json!(row.name));

        // Write the fixture's own slots, then read them back.
        let mut bus = DmxBus::new();
        let now = web_time::Instant::now();
        bus.write(
            1,
            prefix,
            100,
            SourceMode::Htp,
            &[(profile.address, 255)],
            now,
        );
        let sent = fixture_packet_json(&root, &bus, &node, prefix);
        assert_eq!(sent["state"], json!("sending"));
        assert!(
            sent["universe"].is_u64(),
            "the row's universe is reported: {sent:?}"
        );
        assert_eq!(sent["colour"].as_array().map(Vec::len), Some(3));

        // A bypassed node and a pointer at a gone row.
        node.bypass = true;
        assert_eq!(
            fixture_packet_json(&root, &bus, &node, prefix)["state"],
            json!("bypassed")
        );
        node.bypass = false;
        node.config = metal_graph::Config::from_json_str(
            &json!({ "venue": "bar", "fixture": "gone-9" }).to_string(),
        )
        .unwrap();
        assert_eq!(
            fixture_packet_json(&root, &bus, &node, prefix)["state"],
            json!("orphan")
        );
    }

    #[test]
    fn object_forms_parse_from_a_word_or_a_tagged_object() {
        assert_eq!(
            parse_object_form(&json!("box")).unwrap(),
            venue::ObjectForm::Box
        );
        match parse_object_form(&json!({
            "form": "swept",
            "path": [[0, 0, 0], [0, 1, 0]],
            "profile": [[-1, 0], [1, 0], [1, 1], [-1, 1]],
            "caps": true,
            "closed": true,
            "kind": "polyline",
        }))
        .unwrap()
        {
            venue::ObjectForm::Swept {
                path,
                profile,
                caps,
                closed,
                kind,
            } => {
                assert_eq!(path.len(), 2);
                assert_eq!(profile.len(), 4);
                assert!(caps && closed);
                assert_eq!(kind, "polyline");
            }
            other => panic!("not swept: {other:?}"),
        }
        match parse_object_form(&json!({
            "form": "capsule", "radius": 0.3, "height": 2.0,
        }))
        .unwrap()
        {
            venue::ObjectForm::Capsule { radius, height } => {
                assert!((radius - 0.3).abs() < 1e-6 && (height - 2.0).abs() < 1e-6);
            }
            other => panic!("not a capsule: {other:?}"),
        }
        assert!(parse_object_form(
            &json!({ "form": "mesh", "positions": [[0, 0, f64::INFINITY]] })
        )
        .is_err());
        assert!(parse_object_form(&json!("nope")).is_err());
    }

    #[test]
    fn stage_truss_and_dummy_pick_their_default_forms() {
        let dummy = object_op_body(&json!({ "object": "d" }), "dummy");
        assert_eq!(dummy["kind"], json!("dummy"));
        assert_eq!(dummy["form"]["form"], json!("capsule"));

        let stage = object_op_body(&json!({ "object": "s", "size": [4, 0.8, 2] }), "stage");
        assert_eq!(stage["form"]["form"], json!("box"));

        let truss = object_op_body(
            &json!({ "object": "t", "size": [4, 0.4, 0.4], "path": [[0, 0, 0], [4, 0, 0]] }),
            "truss",
        );
        assert_eq!(truss["form"]["form"], json!("swept"));
        assert_eq!(truss["form"]["profile"].as_array().unwrap().len(), 4);

        let given = object_op_body(&json!({ "object": "s", "kind": "riser" }), "stage");
        assert_eq!(given["kind"], json!("riser"));
    }

    /// A write to a room that is not on disk yet creates it (the 400 the
    /// empty room's gallery showed); a read of the same room is still refused,
    /// and a room file that exists is loaded as it is, never replaced.
    #[test]
    fn a_write_to_a_room_not_on_disk_creates_it() {
        let id = format!("fresh-room-{}", std::process::id());
        let root = crate::output::data_root();
        let _ = metal_vfs::remove_file(venue::dir(&root).join(format!("{id}.toml")));

        assert!(room_for(&root, &id, "check").is_err(), "a read stays refused");
        let mut made = room_for(&root, &id, "surface").expect("the first write creates the room");
        assert_eq!(made.venue.id, id);
        assert!(made.surfaces.is_empty() && made.objects.is_empty());
        made.save(&root).expect("the created room saves");
        assert!(room_for(&root, &id, "check").is_ok(), "the saved room reads back");

        let _ = metal_vfs::remove_file(venue::dir(&root).join(format!("{id}.toml")));
        assert!(room_for(&root, "", "screen").is_err(), "an empty id is never a room");
    }

    /// A profile from `data/fixtures/`, for the packet test's channel address.
    fn mkfixture(model: &str) -> crate::fixture::Fixture {
        crate::fixture::get(&crate::output::data_root(), model).expect("the profile")
    }
}
