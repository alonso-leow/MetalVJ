//! §16.7: `POST /overlay` — playing a saved movement over a live layer. See
//! `overlay.rs`'s own module note for the mechanism; this is the one route
//! that arms/stops it, and the one handler here that needs `Ctx::author`
//! (the movement comes out of the absorbed studio's reuse library).
//!
//! # `S.O.5` — `op: "rasterize"`: a timeline lane baked into an overlay
//!
//! *"Rasterizing lanes/edits as overlays: the rasterization and its route are
//! data; the button goes against the shell."* Nothing in it is pixels. A lane is
//! a [`crate::author::models::timeline::ModelSegment`] — a model alive over
//! `[from, to)` ticks with keyframes behind it — and *rasterising* it means
//! resolving those keyframes to one dense float row per tick, which is what
//! [`crate::overlay::Play::ticks`] already is and what `POST /timeline/export`
//! already computes per segment ([`timeline::segment_rows`]). So the whole
//! of `S.O.5` is a window over rows the studio could already produce, saved
//! where the grid already looks:
//!
//! - **The rasterised overlay is data**: a model movement in
//!   `data/library/movements.json` (`{name, model, ticks}`) — the same file,
//!   the same [`crate::author::library::check`], the same `play` body as any
//!   movement lifted by hand.
//! - **Its route is this op**, and it answers with the movement's name so
//!   the caller can arm it; passing `device` + `layer` arms it in the same
//!   call, through [`play`] — one body, two doors (rule 6).
//! - **The button is the grid's own**: the overlays grid lists the library's
//!   movements as buttons already (`panels/lanes.rs`), so a rasterised lane
//!   is a button the moment it is saved, with no shell work.
//!
//! Only the `model` track rasterises. An effect, media, still, text or signal
//! segment is not a row of a model's floats and there is nothing a `Play`
//! could carry for it; the op refuses those by name rather than guessing.

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};
use crate::{layers, overlay, PREVIEW};

/// `[R, G, B]` as three `u8`s, 0-255 — the one shape both `POST /overlay
/// {op:"solid"}` and the `solid_color` tool take. A `Vec<Value>` rather
/// than `[u8; 3]` at the JSON boundary because that is what `serde_json`
/// hands back for an array; this is where it narrows to the type the rest
/// of the overlay substrate (`overlay::SolidColorOverlay::rgb`) actually
/// wants.
fn rgb_bytes(rgb: &[Value]) -> Result<[u8; 3], CallError> {
    if rgb.len() != 3 {
        return Err(CallError::Bad(format!(
            "rgb must be [R, G, B], got {} values",
            rgb.len()
        )));
    }
    let byte = |v: &Value| -> Result<u8, CallError> {
        let n = v
            .as_u64()
            .ok_or_else(|| CallError::Bad("each rgb value must be an integer 0-255".into()))?;
        u8::try_from(n).map_err(|_| CallError::Bad("rgb values must be 0-255".into()))
    };
    Ok([byte(&rgb[0])?, byte(&rgb[1])?, byte(&rgb[2])?])
}

/// `POST /overlay {op:"solid"}`'s `masks`: `{ "<mask id>": [R, G, B] }`,
/// each id one of `device`'s own masks. Absent (or `null`) is no per-mask
/// colour. An id the device does not have is refused by name before anything
/// is armed — a test colour aimed at a contour that is not there would
/// silently show nothing.
fn solid_masks(ctx: &Ctx, device: &str, masks: &Value) -> Result<Vec<(String, [u8; 3])>, CallError> {
    let list = match masks {
        Value::Null => return Ok(Vec::new()),
        Value::Object(list) => list,
        _ => {
            return Err(CallError::Bad(
                "masks must be an object: { \"<mask id>\": [R, G, B] }".into(),
            ))
        }
    };
    let known: Vec<&str> = ctx
        .state
        .outputs
        .find(device)
        .map(|o| o.masks.iter().map(|m| m.id.as_str()).collect())
        .unwrap_or_default();
    let unknown: Vec<&str> = list
        .keys()
        .map(String::as_str)
        .filter(|id| !known.contains(id))
        .collect();
    if !unknown.is_empty() {
        return Err(CallError::Bad(format!(
            "{device:?} has no mask {}",
            unknown.join(", ")
        )));
    }
    list.iter()
        .map(|(id, rgb)| {
            let rgb = rgb.as_array().ok_or_else(|| {
                CallError::Bad(format!("the colour of mask {id} must be [R, G, B]"))
            })?;
            Ok((id.clone(), rgb_bytes(rgb)?))
        })
        .collect()
}

pub fn overlay(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let time = ctx.state.time();
    match body["op"].as_str().unwrap_or_default() {
        "play" => {
            let device = body["device"].as_str().unwrap_or(PREVIEW);
            let Some(lid) = body["layer"].as_str() else {
                return Err(CallError::Bad("which layer".into()));
            };
            let Some(name) = body["movement"].as_str() else {
                return Err(CallError::Bad("which movement".into()));
            };
            let looping = body["loop"].as_bool().unwrap_or(false);
            play(ctx, device, lid, name, looping)?;
        }
        "stop" => {
            let device = body["device"].as_str().unwrap_or(PREVIEW).to_string();
            let lid = body["layer"].as_str().unwrap_or_default().to_string();
            ctx.state.overlays.stop(&device, &lid);
        }
        "stop-all" => {
            ctx.state.overlays.stop_all();
        }
        // `S.O.2`: a solid color for N seconds — the substrate
        // (`overlay::Overlays::start_solid_color`/`is_active`/`report`) has
        // existed since `0fede633` with its own tests; this op is the first
        // door onto it. `rgb` is `[R, G, B]` 0-255, the same shape the
        // `solid_color` tool takes (`agent_tools.rs`).
        //
        // `masks` (`{ "<mask id>": [R, G, B] }`) gives each of the device's
        // masks its own colour — a venue's test projection. With it, `rgb`
        // is the colour outside the listed masks and may be left out
        // (black). Without it this is the one-colour body it always was.
        "solid" => {
            let device = body["device"].as_str().unwrap_or(PREVIEW).to_string();
            let masks = solid_masks(ctx, &device, &body["masks"])?;
            let [r, g, b] = match body["rgb"].as_array() {
                Some(rgb) => rgb_bytes(rgb)?,
                None if !masks.is_empty() => [0, 0, 0],
                None if body["pattern"].is_string() => [255, 255, 255],
                None => {
                    return Err(CallError::Bad("expected `rgb` ([R, G, B], 0-255)".into()))
                }
            };
            let duration = body["duration_seconds"]
                .as_f64()
                .ok_or_else(|| CallError::Bad("expected `duration_seconds`".into()))?
                as f32;
            if duration <= 0.0 {
                return Err(CallError::Bad("duration_seconds must be positive".into()));
            }
            // P-F: `pattern: "grid"` is the edge-blend test pattern —
            // `rgb` (white by default) with a black grid over the device.
            match body["pattern"].as_str() {
                None => ctx
                    .state
                    .overlays
                    .start_solid_masks(device, [r, g, b], masks, time, duration),
                Some("grid") if masks.is_empty() => ctx
                    .state
                    .overlays
                    .start_test_pattern(device, [r, g, b], time, duration),
                Some(other) => {
                    return Err(CallError::Bad(format!(
                        "pattern {other:?}: the only pattern is \"grid\", without `masks`"
                    )))
                }
            }
        }
        // Material glitch trigger (rule 12): a one-shot burst of `mat_glitch`
        // on every generator that draws a material, like an accent hit.
        // `hold: true` keeps it on until `op: "glitch-release"` (the grid's
        // hold edge).
        "glitch" => {
            ctx.state.overlays.hit_glitch(time, body["hold"].as_bool().unwrap_or(false));
        }
        "glitch-release" => {
            ctx.state.overlays.release_glitch();
        }
        "stop-solid" => {
            let device = body["device"].as_str().unwrap_or(PREVIEW);
            ctx.state.overlays.stop_solid_color(device);
        }
        // `S.O.5`: see the module doc. Answers early with what it baked,
        // because the movement's name is what the caller needs next.
        #[cfg(feature = "gpu")]
        "rasterize" => {
            let timeline = body["timeline"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which timeline".into()))?;
            let segment = body["segment"]
                .as_str()
                .or_else(|| body["segment_id"].as_str())
                .ok_or_else(|| CallError::Bad("which segment".into()))?;
            if let Some(track) = body["track"].as_str() {
                if track != "model" {
                    return Err(CallError::Bad(format!(
                        "only the model track rasterises to an overlay — a {track} segment \
                         is not a run of a model's float rows"
                    )));
                }
            }
            let manifest = crate::author::models::schema::Manifest::load(
                &crate::output::data_root().join("schema.json"),
            )
            .map_err(CallError::Internal)?;
            let baked = rasterize_at(
                std::path::Path::new(crate::author::TIMELINES),
                std::path::Path::new(crate::author::models::EXAMPLES_DIR),
                crate::author::LIBRARY,
                &manifest,
                timeline,
                segment,
                body["name"].as_str(),
            )
            .map_err(CallError::Bad)?;
            let mut reply = json!({
                "ok": true,
                "movement": baked.name,
                "model": baked.model,
                "ticks": baked.ticks,
            });
            if let (Some(device), Some(lid)) = (body["device"].as_str(), body["layer"].as_str()) {
                play(
                    ctx,
                    device,
                    lid,
                    &baked.name,
                    body["loop"].as_bool().unwrap_or(false),
                )?;
                reply["overlays"] = ctx.state.overlays.report(time);
            }
            return Ok(reply);
        }
        #[cfg(not(feature = "gpu"))]
        "rasterize" => {
            return Err(CallError::Bad(
                "rasterize needs the studio (gpu build) — timelines are not compiled in".into(),
            ))
        }
        other => return Err(CallError::Bad(format!("no such op: {other}"))),
    }
    Ok(json!({ "overlays": ctx.state.overlays.report(time) }))
}

/// What `rasterize_at` baked: the movement now in the library, and how many
/// ticks it has.
#[cfg(feature = "gpu")]
pub(crate) struct Baked {
    pub name: String,
    pub model: String,
    pub ticks: usize,
}

/// The pure half of `op: "rasterize"`, against explicit paths so a test can
/// point it at a scratch dir (the same split `blackout.rs`'s `apply_at` has).
///
/// Loads `timeline_id`, finds `segment_id` on its model track, resolves the
/// segment's dense rows with [`timeline::segment_rows`] — the exact matrix
/// `POST /timeline/export` writes — windows them to `[from, to)`, and saves
/// them as a model movement named `name` (default `<timeline>/<segment>`)
/// through [`crate::author::library::save`], which is what checks every row
/// against the model's width. A second bake under the same name replaces the
/// first, the library's own "a proposal re-applied is a correction" rule.
#[cfg(feature = "gpu")]
pub(crate) fn rasterize_at(
    timelines: &std::path::Path,
    examples: &std::path::Path,
    library: &str,
    manifest: &crate::author::models::schema::Manifest,
    timeline_id: &str,
    segment_id: &str,
    name: Option<&str>,
) -> Result<Baked, String> {
    use crate::author::models::timeline;

    let Some(tl) = timeline::load(timelines, timeline_id) else {
        return Err(format!("no timeline called {timeline_id:?}"));
    };
    let Some(seg) = tl.model_track.iter().find(|s| s.id == segment_id) else {
        return Err(format!(
            "no model segment {segment_id:?} in {timeline_id:?} — only the model track rasterises"
        ));
    };
    let rows = timeline::segment_rows(examples, &tl, seg, manifest)?;
    let to = seg.to.min(rows.len());
    if seg.from >= to {
        return Err(format!(
            "{segment_id:?} covers no ticks ({}..{})",
            seg.from, seg.to
        ));
    }
    // Re-based to tick 0, the way `library::slice` re-bases a lifted run: a
    // movement is a shape, not a position.
    let ticks: Vec<Value> = rows[seg.from..to].iter().map(|r| json!(r)).collect();
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{timeline_id}/{segment_id}"));
    let movement = json!({ "name": name, "model": seg.model, "ticks": ticks });
    let widths = |id: &str| manifest.model(id).map(|m| m.width());
    crate::author::library::save(library, &movement, &widths)?;
    Ok(Baked {
        name,
        model: seg.model.clone(),
        ticks: to - seg.from,
    })
}

/// The play half, shared by `POST /overlay {op:"play"}` and `POST
/// /trigger/<id>` firing an `overlay-<movement>` target — one route's body,
/// two doors (rule 6). Resolves the movement out of the studio's reuse
/// library and arms it on the named layer.
pub(crate) fn play(
    ctx: &mut Ctx,
    device: &str,
    lid: &str,
    name: &str,
    looping: bool,
) -> Result<(), CallError> {
    let time = ctx.state.time();
    let air = ctx.state.airing(ctx.sim, time);
    let models = ctx.paint.model_count();
    let Some(stack) = ctx.state.stack(device) else {
        return Err(CallError::Bad("no such device".into()));
    };
    match stack.mode {
        layers::DeviceMode::Disabled => {
            return Err(CallError::Bad(
                "this device is off — nothing on it is being drawn to play over".into(),
            ))
        }
        layers::DeviceMode::Automix => {
            return Err(CallError::Bad(
                "this device is on automix, so it is not drawing its own layers — \
                 put it on propio (or a collection) first"
                    .into(),
            ))
        }
        _ => {}
    }
    let routed = ctx.state.routed_to(device);
    let Some(layer) = routed.iter().find(|l| l.id == lid) else {
        return Err(CallError::Bad("no such layer".into()));
    };
    let showing = layer.showing(&air, models);

    let (model_id, ticks) = match ctx.author.movement(name) {
        Ok(m) => m,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let Some(model) = ctx.paint.ids().iter().position(|id| *id == model_id) else {
        return Err(CallError::Bad(format!(
            "{name:?} is for {model_id:?}, which this build has not got"
        )));
    };
    let width = ctx.paint.schemas()[model].width();
    if let Some((i, row)) = ticks.iter().enumerate().find(|(_, r)| r.len() != width) {
        return Err(CallError::Bad(format!(
            "{name:?}: tick {i} is {} floats, {model_id} takes {width}",
            row.len()
        )));
    }
    if ticks.is_empty() {
        return Err(CallError::Bad(format!("{name:?} has no ticks in it")));
    }
    if showing != model {
        return Err(CallError::Bad(format!(
            "\u{201c}{}\u{201d} is drawing {}, and {name:?} is a run of {model_id} rows — \
             point the source at {model_id} first",
            layer.name,
            ctx.paint.names()[showing],
        )));
    }

    ctx.state.overlays.start(overlay::Play {
        device: device.to_string(),
        layer: lid.to_string(),
        movement: name.to_string(),
        model,
        ticks,
        started: time,
        looping,
        mask: ctx.author.movement_mask(name),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The same fixture shape `transmit::tests::fixture` builds — see that
    /// module's own doc comment for why `Author::new()` needs the cwd dance.
    /// Neither `solid` nor `stop-solid` reads `ctx.author`, but `Ctx` always
    /// carries one.
    fn fixture() -> (
        Box<dyn crate::paint::Paint>,
        crate::State,
        crate::listeners::Listeners,
        crate::Author,
    ) {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        (paint, state, sim, author)
    }

    fn body(v: Value) -> Params {
        Params {
            query: HashMap::new(),
            body: v.to_string().into_bytes(),
            tail: None,
        }
    }

    /// `S.O.2`: `POST /overlay {op:"solid"}` reaches
    /// `overlay::Overlays::start_solid_color`, and `/state`'s own
    /// `report()` (already covered by `metal_perform::overlay`'s tests) sees
    /// it as `active` until `duration_seconds` runs out.
    #[test]
    fn a_solid_op_starts_a_solid_color_overlay() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let reply = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "device": "preview", "rgb": [255, 0, 0], "duration_seconds": 2.0 })),
        )
        .expect("solid should start");
        let entries = reply["overlays"].as_array().expect("a list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["type"], json!("solid_color"));
        assert_eq!(entries[0]["rgb"], json!([255, 0, 0]));
        assert!(entries[0]["active"].as_bool().unwrap());

        let reply = overlay(
            &mut ctx,
            &body(json!({ "op": "stop-solid", "device": "preview" })),
        )
        .expect("stop-solid should succeed");
        assert!(
            reply["overlays"].as_array().unwrap().is_empty(),
            "stopped — nothing left in the air"
        );
    }

    /// Venue test colours through the route: `masks` arms one colour per
    /// mask (the report lists them), `rgb` may be left out (black outside
    /// the masks), a mask the device does not have is refused by name, and
    /// the one-colour body reports exactly what it always did.
    #[test]
    fn a_solid_op_takes_a_colour_per_mask() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut wall = crate::output::Output::new("wall", "Wall", 64, 36);
        for id in ["bar.panel-left", "bar.panel-right"] {
            wall.masks.push(crate::output::Mask {
                id: id.into(),
                name: id.into(),
                points: vec![[0.1, 0.1], [0.4, 0.1], [0.4, 0.4]],
                ..Default::default()
            });
        }
        state.outputs.list.push(wall);
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let reply = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "device": "wall", "duration_seconds": 5.0,
                          "masks": { "bar.panel-left": [255, 120, 40], "bar.panel-right": [40, 140, 255] } })),
        )
        .expect("a per-mask solid");
        let entry = &reply["overlays"][0];
        assert_eq!(entry["rgb"], json!([0, 0, 0]), "outside the masks: black");
        assert_eq!(
            entry["masks"],
            json!({ "bar.panel-left": [255, 120, 40], "bar.panel-right": [40, 140, 255] })
        );

        let refused = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "device": "wall", "duration_seconds": 5.0,
                          "masks": { "bar.panel-left": [1, 2, 3], "nope": [1, 2, 3] } })),
        );
        match refused {
            Err(CallError::Bad(msg)) => assert!(msg.contains("nope"), "{msg}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let bad_colour = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "device": "wall", "duration_seconds": 5.0,
                          "masks": { "bar.panel-left": [300, 0, 0] } })),
        );
        assert!(matches!(bad_colour, Err(CallError::Bad(_))));
        let no_colour = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "device": "wall", "duration_seconds": 5.0 })),
        );
        assert!(matches!(no_colour, Err(CallError::Bad(_))), "no masks still needs rgb");
        assert_eq!(
            ctx.state.overlays.solid_masks("wall", ctx.state.time()).len(),
            2,
            "the refusals armed nothing over the first solid"
        );

        let reply = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "device": "wall", "rgb": [9, 9, 9], "duration_seconds": 5.0 })),
        )
        .expect("the one-colour body");
        let entry = &reply["overlays"][0];
        assert_eq!(entry["rgb"], json!([9, 9, 9]));
        assert!(entry.get("masks").is_none(), "{entry}");
    }

    /// A malformed `rgb` (wrong length, out-of-range byte) is refused before
    /// anything is armed — the same "reject before drawing" posture `play`'s
    /// own guards take.
    #[test]
    fn a_glitch_op_fires_a_burst_and_release_ends_a_hold() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        overlay(&mut ctx, &body(json!({ "op": "glitch" }))).expect("glitch");
        let t = ctx.state.time();
        assert!(ctx.state.overlays.glitch_level(t) > 0.0, "the burst is in the air");
        assert_eq!(ctx.state.overlays.glitch_level(t + 5.0), 0.0, "and gone a few seconds later");
        overlay(&mut ctx, &body(json!({ "op": "glitch", "hold": true }))).expect("hold");
        assert_eq!(ctx.state.overlays.glitch_level(ctx.state.time() + 60.0), 1.0, "a hold stays on");
        overlay(&mut ctx, &body(json!({ "op": "glitch-release" }))).expect("release");
        assert_eq!(ctx.state.overlays.glitch_level(ctx.state.time() + 60.0), 0.0);
    }

    #[test]
    fn a_solid_op_with_bad_rgb_is_refused() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let too_few = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "rgb": [255, 0], "duration_seconds": 1.0 })),
        );
        assert!(matches!(too_few, Err(CallError::Bad(_))));

        let out_of_range = overlay(
            &mut ctx,
            &body(json!({ "op": "solid", "rgb": [999, 0, 0], "duration_seconds": 1.0 })),
        );
        assert!(matches!(out_of_range, Err(CallError::Bad(_))));
    }

    /// A scratch studio for `rasterize_at`: one timeline whose only model
    /// segment covers ticks `[4, 12)` of the manifest's first model, with
    /// two keyframes at 4 and 11 so the baked rows actually move.
    #[cfg(feature = "gpu")]
    fn studio() -> (
        std::path::PathBuf,
        crate::author::models::schema::Manifest,
        String,
    ) {
        use crate::author::models::{example, timeline};
        let root = std::env::temp_dir().join(format!(
            "fw-rasterize-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = metal_vfs::remove_dir_all(&root);
        metal_vfs::create_dir_all(root.join("timelines")).unwrap();
        let manifest = crate::author::models::schema::Manifest::load(
            &crate::output::data_root().join("schema.json"),
        )
        .expect("the repo's schema.json");
        let schema = &manifest.models[0];
        let model = schema.id.clone();

        let mut tl = timeline::Timeline::new("show", "show", "song", 16);
        tl.model_track.push(timeline::ModelSegment {
            id: "seg-1".into(),
            from: 4,
            to: 12,
            model: model.clone(),
            layer: 0,
            mix: None,
        });
        metal_vfs::write(
            root.join("timelines/show.json"),
            serde_json::to_string(&tl).unwrap(),
        )
        .unwrap();

        let mut lo = schema.defaults();
        lo[0] = 0.0;
        let mut hi = schema.defaults();
        hi[0] = 1.0;
        example::save(
            &root.join("animations"),
            example::Example {
                model: model.clone(),
                song: "song".into(),
                rate: metal_schema::TickRate::CANONICAL.hz() as f32,
                ticks: vec![
                    example::Tick {
                        tick: 4,
                        floats: lo,
                    },
                    example::Tick {
                        tick: 11,
                        floats: hi,
                    },
                ],
            },
            schema,
            None,
        )
        .unwrap();
        (root, manifest, model)
    }

    /// `S.O.5`: a model segment bakes to exactly its `[from, to)` ticks of
    /// the same dense rows `POST /timeline/export` computes, lands in the
    /// library as a model movement the grid can already play, and a second
    /// bake under the same name replaces rather than doubles.
    #[cfg(feature = "gpu")]
    #[test]
    fn rasterize_bakes_a_segment_into_a_library_movement() {
        let (root, manifest, model) = studio();
        let library = root.join("library/movements.json");
        let library = library.to_str().unwrap();

        let baked = rasterize_at(
            &root.join("timelines"),
            &root.join("animations"),
            library,
            &manifest,
            "show",
            "seg-1",
            None,
        )
        .expect("bakes");
        assert_eq!(baked.name, "show/seg-1");
        assert_eq!(baked.model, model);
        assert_eq!(baked.ticks, 8, "[4, 12) is eight ticks");

        let m = crate::author::library::get(library, "show/seg-1").expect("saved");
        assert_eq!(m.model.as_deref(), Some(model.as_str()));
        assert_eq!(m.ticks.len(), 8);
        assert!(!m.favourite, "a star is the operator's, never a bake's");
        let first = m.ticks[0][0].as_f64().unwrap();
        let last = m.ticks[7][0].as_f64().unwrap();
        assert_eq!(first, 0.0, "tick 4 is the first keyframe, re-based to 0");
        assert_eq!(last, 1.0, "tick 11 is the last");
        let mid = m.ticks[3][0].as_f64().unwrap();
        assert!(
            mid > 0.0 && mid < 1.0,
            "between them it interpolates: {mid}"
        );

        // The export's rows are the same rows — one function, not two.
        let tl = crate::author::models::timeline::load(&root.join("timelines"), "show").unwrap();
        let rows = crate::author::models::timeline::segment_rows(
            &root.join("animations"),
            &tl,
            &tl.model_track[0],
            &manifest,
        )
        .unwrap();
        let baked_rows: Vec<Vec<f32>> = m
            .ticks
            .iter()
            .map(|t| {
                t.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect()
            })
            .collect();
        assert_eq!(baked_rows, rows[4..12].to_vec());

        rasterize_at(
            &root.join("timelines"),
            &root.join("animations"),
            library,
            &manifest,
            "show",
            "seg-1",
            Some("show/seg-1"),
        )
        .expect("bakes again");
        assert_eq!(
            crate::author::library::load(library).len(),
            1,
            "replaced, not doubled"
        );

        metal_vfs::remove_dir_all(&root).ok();
    }

    /// Wrong timeline, wrong segment: refused by name, nothing written.
    #[cfg(feature = "gpu")]
    #[test]
    fn rasterize_refuses_what_it_cannot_find() {
        let (root, manifest, _) = studio();
        let library = root.join("library/movements.json");
        let library = library.to_str().unwrap();
        let tls = root.join("timelines");
        let ex = root.join("animations");
        assert!(rasterize_at(&tls, &ex, library, &manifest, "nope", "seg-1", None).is_err());
        assert!(rasterize_at(&tls, &ex, library, &manifest, "show", "seg-9", None).is_err());
        assert!(
            crate::author::library::load(library).is_empty(),
            "nothing written"
        );
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// The op's own guards, through the handler: a non-model track is refused
    /// before any file is read, and a body missing its names is refused too.
    #[cfg(feature = "gpu")]
    #[test]
    fn a_rasterize_op_only_takes_the_model_track() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let refused = overlay(
            &mut ctx,
            &body(
                json!({ "op": "rasterize", "timeline": "show", "segment": "fx-1", "track": "effect" }),
            ),
        );
        match refused {
            Err(CallError::Bad(msg)) => assert!(msg.contains("model track"), "{msg}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let nameless = overlay(
            &mut ctx,
            &body(json!({ "op": "rasterize", "timeline": "show" })),
        );
        assert!(matches!(nameless, Err(CallError::Bad(_))));
    }
}
