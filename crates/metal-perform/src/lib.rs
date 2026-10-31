//! Resolving a generator's `fx.*` chain (`metal_graph::effects`) into the
//! `metal_schema::Post` the GPU eventually wants — the bridge between "what the
//! operator wired" and "what a row of floats says", the same kind of seam
//! `drive.rs` is for the digest.
//!
//! **Here and not in `metal-graph`** (LAY.6): it needs `metal_schema`'s
//! [`metal_schema::Post::set_amount`]/[`metal_schema::effect_index`] *and* the row
//! a generator was already driven with — `metal-graph` knows the chain exists,
//! not what a float in it is worth. It used to live in `metal-dash` for that
//! reason; `metal-perform` is where the rest of the engine (resolve → compile →
//! execute → output, LAY.9) lands, and moving this first is what keeps the
//! resolver off the binary when it does.
//!
//! **The painter boundary is here too** (LAY.9/LAY.10): [`paint::Paint`] and
//! its provided [`paint::Paint::run`] — the execute → output half of the
//! pipeline — plus [`stand::Stand`], the CPU painter, and the `gpu`-gated
//! [`paint::Gpu`] wrapper. `metal-dash` only chooses one and hands it frames;
//! everything downstream of the choice takes `&mut dyn Paint` and never asks
//! which it got. That is what lets the conformance corpus and `stand` run
//! without the HTTP/winit half of the binary, and what makes "one execution
//! path for both builds" a property of the types rather than a discipline.

pub mod artnet;
pub mod atomic;
pub mod backend;
pub mod bind;
pub mod capture;
pub mod clock;
pub mod compose;
pub mod devices;
pub mod edge_blend;
pub mod digest;
pub mod dmx_bus;
pub mod drive;
pub mod engine;
pub mod face;
pub mod escena;
pub mod export;
pub mod fixture;
// D2 (show-control lote): the GDTF importer — a `.gdtf` zip to a
// `data/fixtures/<model>.toml` profile in the `fixture` model.
pub mod gdtf;
pub mod gpu_frame;
#[cfg(feature = "gpu")]
pub mod gpu_compose;
#[cfg(feature = "gpu")]
pub mod gpu_cover;
#[cfg(feature = "gpu")]
pub mod gpu_edge;
#[cfg(feature = "gpu")]
pub mod gpu_fx;
#[cfg(feature = "gpu")]
pub mod gpu_hist;
#[cfg(all(test, feature = "gpu"))]
mod g3_tests;
pub mod layers;
pub mod listeners;
pub mod live;
pub mod looper;
pub mod motion;
pub mod node_layer;
pub mod output;
pub mod overlay;
pub mod pattern;
pub mod paint;
pub mod worker_proto;
pub mod reel;
pub mod reel_batch;
pub mod router;
pub mod sacn;
pub mod scenes;
pub mod script;
pub mod script_preset;
pub mod sequence;
pub mod source;
pub mod stand;
pub mod switch;
pub mod text;
#[cfg(feature = "gpu")]
pub mod text_gpu;
pub mod timecode;
pub mod transition;
#[cfg(feature = "gpu")]
pub mod transition_gpu;
pub mod trigger;
pub mod venue;
pub mod forge;
pub mod grade;
pub mod fx;
pub mod wire;

pub use engine::*;
pub use paint::{encode_jpeg, painter, Paint, RunOutput};
pub use stand::Stand;

#[cfg(feature = "gpu")]
pub use paint::Gpu;

use metal_graph::effects::ControlSource;
use metal_schema::{EffectState, EffectStep, Post};

/// Bakes one advanced `fx.*` node's floats into `post.advanced` — the block
/// `metal_schema::read_post` mirrors out of the row's tail and both the GPU pass
/// and the CPU compositor read. The advanced mirror of what
/// [`resolved_chain`] does for the 14 common effects, with the same
/// off / reactive / fixed rule:
///
/// - **Fixed**: each parameter is read from the node's own config key
///   (`fx.grade` holds `grade_temperature`, not a single `fixed` — the same
///   shape `fx.move` uses), missing keys falling back to the spec's declared
///   neutral, so a freshly materialized node is the neutral grade.
/// - **Reactive from `RawIngest`**: each float comes off the digest column
///   `effect_column_map` maps its flat `EFFECTS_FLOATS` position to.
/// - **Reactive from the row**: left alone — the baseline `drive::row` (which
///   already drives the advanced tail) is the router's answer.
/// - **Off**: every float pinned to its declared neutral, the advanced mirror
///   of `set_amount(idx, 0.0)`.
fn resolve_advanced_effect(
    graph: &metal_graph::Graph,
    fx_id: &str,
    name: &str,
    post: &mut Post,
    raw_digest: &[f32],
) {
    let (Some(floats), Some(span)) = (
        metal_schema::advanced_effect_floats(name),
        metal_schema::advanced_effect_span(name),
    ) else {
        return;
    };
    let base = span.start - metal_schema::CHAIN_FLOATS;
    let config: serde_json::Value = graph
        .node(fx_id)
        .and_then(|n| serde_json::from_str(&n.config.to_json_str()).ok())
        .unwrap_or(serde_json::Value::Null);

    match metal_graph::effects::state_of(graph, fx_id) {
        EffectState::Fixed(_) => {
            for (i, spec) in floats.iter().enumerate() {
                let v = config
                    .get(spec.name)
                    .and_then(serde_json::Value::as_f64)
                    .map(|v| v as f32)
                    .unwrap_or(spec.default)
                    .clamp(spec.min, spec.max);
                if let Some(slot) = post.advanced.get_mut(base + i) {
                    *slot = v;
                }
            }
        }
        EffectState::Reactive => {
            if metal_graph::effects::control_source(graph, fx_id) == Some(ControlSource::RawIngest) {
                for (i, spec) in floats.iter().enumerate() {
                    let flat = span.start + i;
                    if let Some(&col) = metal_schema::effect_column_map().get(flat) {
                        if let Some(&raw) = raw_digest.get(col) {
                            if let Some(slot) = post.advanced.get_mut(base + i) {
                                *slot = raw.clamp(spec.min, spec.max);
                            }
                        }
                    }
                }
            }
        }
        EffectState::Off => {
            for (i, spec) in floats.iter().enumerate() {
                if let Some(slot) = post.advanced.get_mut(base + i) {
                    *slot = spec.default;
                }
            }
        }
    }
}

/// Builds this generator's resolved effect chain and bakes every `fixed`/
/// raw-ingest amount into `post` on top of whatever `drive::row` already
/// computed for it — the per-node, per-graph mirror of `engine.rs`'s
/// `apply_node_post`: that function fixes floats a `Layer` pinned; this
/// fixes floats an `fx.*` node the operator actually materialized pinned,
/// in the order that node sits in the graph.
///
/// `post` should be [`metal_schema::read_post`] of the row this generator was
/// just driven with — its fourteen named fields are already the reactive
/// baseline (today's deterministic `drive::row` mapping, standing in for
/// axis (a) — "the AI models" — until a trained router produces one of
/// its own). This function only ever *overrides* that baseline, and only
/// for the effects actually present in the chain:
///
/// - **Off** (not in the chain — `metal_graph::effects::chain` never returns
///   it): untouched, so a row with no operator opinion at all still shows
///   whatever `drive::row` put there. That is the "third scenario" the user
///   named as out of scope for now — this function does not have to make
///   it correct, only not corrupt it.
/// - **Fixed(v)**: `post.set_amount` overwrites the baseline with `v`.
/// - **Reactive, `Router`-sourced**: left alone — the baseline *is* the
///   router's answer.
/// - **Reactive, `RawIngest`-sourced**: overwritten with `raw_digest`'s own
///   column, straight off [`metal_schema::effect_column_map`], no shaping —
///   axis (b), *"with no processing at all"*. `raw_digest` is the frame's
///   `.raw` array (`crate::digest::Frame::raw`); a short or empty slice
///   (an asset-grid render with no live digest, or a test that does not
///   care about this path) just leaves the baseline in place rather than
///   panicking.
///
/// `wake` — `metal_schema::effect_index` returns `None` for it,
/// on purpose (`the design notes` §3.2/Set F) — is walked like any
/// other chain member, but there is no `Post` field for it to land an
/// amount in (see [`metal_schema::effect_index`]'s own doc comment), so it
/// cannot go through `post.set_amount`/`post.amount` the ten common effects
/// use. `metal_schema::footage_chain_kind` is the sibling lookup for exactly
/// this name, and what a step naming it resolves to depends on state the
/// same way any other effect's does:
///
/// - **Fixed(v)**: resolves straight to `v` — the only state with anywhere
///   to read an amount *from*, since there is no row field to bake it into
///   first.
/// - **Reactive**: contributes no step, same as before this function knew
///   this name at all. Both axes a common effect's `Reactive` can read
///   from are closed here: `Router`-sourced would mean "trust the row
///   `drive::row` already computed", and there is no row float to have
///   computed one; `RawIngest`-sourced would mean a digest column off
///   `effect_column_map`, which is indexed by `EFFECTS_FLOATS` position and
///   has none for this kind. Guessing a number neither source actually
///   produced would be worse than the gap.
/// The chain one generator's row resolves to, exactly as `Paint::run` resolves
/// it: the graph's `fx.*` steps in order, with the advanced block of `row`
/// written back so a `fixed` grade node reaches the picture. Empty for a row
/// too short to carry effects. The local pass and a remote tile both call
/// this, so the two cannot disagree.
pub fn chain_for_row(
    graph: &metal_graph::Graph,
    gen_node: &str,
    row: &mut [f32],
    raw_digest: &[f32],
) -> Vec<EffectStep> {
    if row.len() < metal_schema::EFFECTS_WIDTH {
        return Vec::new();
    }
    let mut post = metal_schema::read_post(row);
    let resolved = resolved_chain(graph, gen_node, &mut post, raw_digest);
    // The advanced effects (`fx.grade` and the six after it) read their
    // numbers from the row's trailing block; write the resolved mirror back.
    let at = row.len() - metal_schema::EFFECTS_WIDTH;
    for (i, v) in post.advanced.iter().enumerate() {
        if let Some(slot) = row.get_mut(at + metal_schema::CHAIN_FLOATS + i) {
            *slot = *v;
        }
    }
    // The `mat.*` chain the operator wired downstream, baked into the row's
    // trailing `material` group so the model's shader reads the graph's own
    // answer (rule 4) — the material counterpart of the advanced write-back
    // above. Only the `part: "*"` node has a whole-row slot; a per-part node
    // stays in [`resolved_materials`] for the surface path.
    let material_at =
        row.len() - metal_schema::EFFECTS_WIDTH + metal_schema::CHAIN_FLOATS + metal_schema::N_ADVANCED;
    for material in resolved_materials(graph, gen_node, row) {
        if material.part != metal_graph::materials::PART_ANY {
            continue;
        }
        for (i, v) in material.floats.iter().enumerate() {
            if let Some(slot) = row.get_mut(material_at + i) {
                *slot = *v;
            }
        }
    }
    resolved
}

/// The material a generator's `mat.*` chain resolves to, read back off the
/// graph (rule 4) with the row's own `material` group as the starting point.
/// Empty for a row too short to carry the group or a generator with no
/// material wired — the same contract [`chain_for_row`] has for effects.
pub fn resolved_materials(
    graph: &metal_graph::Graph,
    gen_node: &str,
    row: &[f32],
) -> Vec<metal_graph::materials::ResolvedMaterial> {
    if row.len() < metal_schema::EFFECTS_WIDTH {
        return Vec::new();
    }
    let base = metal_schema::read_material(row).unwrap_or(&[]);
    metal_graph::materials::resolve(graph, gen_node, base)
}

pub fn resolved_chain(
    graph: &metal_graph::Graph,
    gen_node: &str,
    post: &mut Post,
    raw_digest: &[f32],
) -> Vec<EffectStep> {
    let mut chain = Vec::new();
    for fx_id in metal_graph::effects::chain(graph, gen_node) {
        let Some(name) = metal_graph::effects::effect_name(graph, &fx_id) else {
            continue;
        };

        if let Some(kind) = metal_schema::footage_chain_kind(&name) {
            if let EffectState::Fixed(v) = metal_graph::effects::state_of(graph, &fx_id) {
                chain.push(EffectStep {
                    effect: kind,
                    amount: v,
                    slot: 0,
                });
            }
            continue;
        }

        // An advanced effect (grade, texture, clarity, dehaze, denoise,
        // optics, geometry) is not a common chain step: its floats live in
        // the row's trailing advanced block, which `read_post` already
        // mirrors into `post.advanced`. Bake the node's own opinion into
        // that mirror here and move on — see `resolve_advanced_effect`.
        if metal_schema::advanced_effect_floats(&name).is_some() {
            resolve_advanced_effect(graph, &fx_id, &name, post, raw_digest);
            // ...except an advanced group that is also a chain step (`fx.fm`):
            // its first float is the step's amount, so it takes its place in
            // the chain here, at the node's position. Off (amount 0) spends
            // no slot.
            if let Some(kind) = metal_schema::advanced_chain_kind(&name) {
                let amount = post.amount(kind as usize);
                if amount != 0.0 {
                    chain.push(EffectStep { effect: kind, amount, slot: 0 });
                }
            }
            continue;
        }

        let Some(idx) = metal_schema::effect_index(&name) else {
            continue;
        };

        match metal_graph::effects::state_of(graph, &fx_id) {
            EffectState::Fixed(v) => post.set_amount(idx, v),
            EffectState::Reactive => {
                if metal_graph::effects::control_source(graph, &fx_id)
                    == Some(ControlSource::RawIngest)
                {
                    if let Some(&col) = metal_schema::effect_column_map().get(idx) {
                        if let Some(&raw) = raw_digest.get(col) {
                            let spec = &metal_schema::EFFECTS_FLOATS[idx];
                            post.set_amount(idx, raw.clamp(spec.min, spec.max));
                        }
                    }
                }
                // Router-sourced: the baseline drive::row already computed
                // is the router's answer — nothing to override.
            }
            // A bypassed `fx.*` node (O.28: a switch turned it off). The
            // baseline `drive::row` may still carry the legacy amount for
            // this effect; "skip this step" means the picture shows none of
            // it, so the amount is pinned to its neutral zero, not left as
            // whatever the row said.
            EffectState::Off => post.set_amount(idx, 0.0),
        }

        chain.push(EffectStep {
            effect: idx as u32,
            amount: post.amount(idx),
            slot: 0,
        });
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Domain, Endpoint, Graph};

    fn wire_gen_out(g: &mut Graph) -> (metal_graph::NodeId, metal_graph::NodeId) {
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        (gen, out)
    }

    #[test]
    fn an_empty_chain_resolves_to_no_steps_and_leaves_the_row_untouched() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);
        assert!(chain.is_empty());
        assert_eq!(post, Post::default());
    }

    #[test]
    fn a_fixed_effect_resolves_to_its_constant_amount() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "vignette",
            EffectState::Fixed(0.6),
            ControlSource::RawIngest,
        )
        .unwrap();

        let mut post = Post::default();
        post.vignette = 0.05; // whatever drive::row happened to compute
        let chain = resolved_chain(&g, &gen, &mut post, &[]);

        assert_eq!(chain.len(), 1);
        let idx = metal_schema::effect_index("vignette").unwrap();
        assert_eq!(chain[0].effect, idx as u32);
        assert_eq!(
            chain[0].amount, 0.6,
            "fixed must overwrite whatever the row computed"
        );
        assert_eq!(post.vignette, 0.6);
    }

    #[test]
    fn a_reactive_router_sourced_effect_keeps_the_rows_own_value() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "shake",
            EffectState::Reactive,
            ControlSource::Row,
        )
        .unwrap();

        let mut post = Post::default();
        post.shake = 0.37; // what drive::row already decided
        let chain = resolved_chain(&g, &gen, &mut post, &[0.9, 0.9, 0.9, 0.9]);

        assert_eq!(
            post.shake, 0.37,
            "router-sourced reactive must not be touched by raw_digest"
        );
        assert_eq!(chain[0].amount, 0.37);
    }

    #[test]
    fn a_reactive_raw_ingest_effect_reads_straight_off_the_digest_column_not_the_row() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let (gen, _out) = wire_gen_out(&mut g);
        let _ = &audio;
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "shake",
            EffectState::Reactive,
            ControlSource::RawIngest,
        )
        .unwrap();

        let mut post = Post::default();
        post.shake = 0.1; // what drive::row computed — must be overridden
                          // shake's column, per metal_schema::effect_column_map, is column 0.
        let raw = [0.8, 0.0, 0.0, 0.0];
        let chain = resolved_chain(&g, &gen, &mut post, &raw);

        assert_eq!(
            post.shake, 0.8,
            "raw ingest must read the digest column directly"
        );
        assert_eq!(chain[0].amount, 0.8);
    }

    #[test]
    fn raw_ingest_clamps_into_the_effects_own_declared_range() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "zoom",
            EffectState::Reactive,
            ControlSource::RawIngest,
        )
        .unwrap();

        let mut post = Post::default();
        // zoom's column is column 0 too (per effect_column_map); feed it a
        // value outside zoom's declared [-1.0, 1.0] range.
        let raw = [4.0, 0.0, 0.0, 0.0];
        resolved_chain(&g, &gen, &mut post, &raw);

        assert_eq!(
            post.zoom, 1.0,
            "must clamp to the effect's own declared max"
        );
    }

    #[test]
    fn a_short_raw_digest_leaves_the_rows_baseline_in_place_rather_than_panicking() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "lorenz",
            EffectState::Reactive,
            ControlSource::RawIngest,
        )
        .unwrap();

        let mut post = Post::default();
        post.lorenz = 0.22;
        let chain = resolved_chain(&g, &gen, &mut post, &[]); // no digest at all

        assert_eq!(
            post.lorenz, 0.22,
            "an empty raw digest must not panic or zero the row"
        );
        assert_eq!(chain[0].amount, 0.22);
    }

    /// The gap Set F closes: `metal_schema::effect_index` still returns
    /// `None` for `wake` (on purpose — it has no row
    /// float), but `metal_schema::footage_chain_kind` gives `resolved_chain`
    /// somewhere else to send it, so a `fx.estela` node the
    /// operator actually wired now shows up in the resolved chain instead
    /// of vanishing the way it used to.
    #[test]
    fn a_fixed_estela_effect_in_the_chain_now_resolves_to_its_own_chain_kind() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "wake",
            EffectState::Fixed(0.55),
            ControlSource::RawIngest,
        )
        .unwrap();
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "vignette",
            EffectState::Fixed(0.4),
            ControlSource::RawIngest,
        )
        .unwrap();

        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);

        assert_eq!(
            chain,
            vec![
                EffectStep {
                    effect: metal_schema::CHAIN_KIND_ESTELA,
                    amount: 0.55,
                    slot: 0
                },
                EffectStep {
                    effect: metal_schema::effect_index("vignette").unwrap() as u32,
                    amount: 0.4,
                    slot: 0,
                },
            ],
            "estela in chain order before vignette, each reading its amount from its own state"
        );
        // Estela has no `Post` field — vignette's amount is the only one
        // that could possibly have landed there.
        let mut expected_post = Post::default();
        expected_post.vignette = 0.4;
        assert_eq!(post, expected_post);
    }

    /// A chain mixing a footage promotion with common effects on either side
    /// of it — the shape B1/B2 exist to make cheap — must resolve all three,
    /// each still reading its amount from wherever that effect's amount
    /// actually lives, in the order the graph puts them in.
    #[test]
    fn footage_and_common_effects_resolve_together_in_graph_order() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "shake",
            EffectState::Fixed(0.2),
            ControlSource::RawIngest,
        )
        .unwrap();
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "wake",
            EffectState::Fixed(0.8),
            ControlSource::RawIngest,
        )
        .unwrap();
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "crt",
            EffectState::Fixed(0.3),
            ControlSource::RawIngest,
        )
        .unwrap();

        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);

        assert_eq!(
            chain,
            vec![
                EffectStep {
                    effect: metal_schema::effect_index("shake").unwrap() as u32,
                    amount: 0.2,
                    slot: 0,
                },
                EffectStep {
                    effect: metal_schema::CHAIN_KIND_ESTELA,
                    amount: 0.8,
                    slot: 0
                },
                EffectStep {
                    effect: metal_schema::effect_index("crt").unwrap() as u32,
                    amount: 0.3,
                    slot: 0,
                },
            ]
        );
    }

    /// An advanced `fx.grade` node's own config keys — not a single `fixed` —
    /// must reach `Post.advanced`, the block `read_post` mirrors and both the
    /// CPU compositor and the GPU pass read. This is the bake task's core.
    #[test]
    fn a_fixed_advanced_grade_node_bakes_its_config_into_post_advanced() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "grade",
            EffectState::Fixed(0.0),
            ControlSource::Row,
        )
        .expect("materializes");
        let grade = metal_graph::effects::chain(&g, &gen)
            .into_iter()
            .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("grade"))
            .expect("the grade node");
        crate::bind::set_config_number(&mut g, &grade, "grade_exposure", 60.0);
        crate::bind::set_config_number(&mut g, &grade, "grade_saturation", -25.0);

        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);

        // The grade is not a common step: no chain entry, and the snapshot of
        // the row baseline is otherwise untouched.
        assert!(chain.is_empty(), "an advanced effect is not a common chain step");
        let span = metal_schema::advanced_effect_span("grade").unwrap();
        let base = span.start - metal_schema::CHAIN_FLOATS;
        assert_eq!(post.advanced[base + 2], 60.0, "grade_exposure");
        assert_eq!(post.advanced[base + 9], -25.0, "grade_saturation");
        // An unset key falls back to the spec's declared neutral, so a fresh
        // node is not a surprise.
        assert_eq!(
            post.advanced[base + 3],
            metal_schema::GRADE_FLOATS[3].default
        );
    }

    /// `fx.fm` is an advanced group *and* a chain step: its config reaches
    /// `Post.advanced` and a step with the first float as amount joins the
    /// chain, at zero amount no step at all.
    #[test]
    fn a_fixed_fm_node_is_a_chain_step_with_its_cant_as_the_amount() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(&mut g, &gen, "fm", EffectState::Fixed(0.0), ControlSource::Row)
            .expect("materializes");
        let fm = metal_graph::effects::chain(&g, &gen)
            .into_iter()
            .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("fm"))
            .expect("the fm node");

        let mut post = Post::default();
        assert!(resolved_chain(&g, &gen, &mut post, &[]).is_empty(), "amount 0 spends no step");

        crate::bind::set_config_number(&mut g, &fm, "fx_fm_cant", 0.7);
        crate::bind::set_config_number(&mut g, &fm, "fx_fm_indice", 5.0);
        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);
        let kind = metal_schema::advanced_chain_kind("fm").unwrap();
        assert_eq!(chain, vec![EffectStep { effect: kind, amount: 0.7, slot: 0 }]);
        let base = metal_schema::advanced_effect_span("fm").unwrap().start - metal_schema::CHAIN_FLOATS;
        assert_eq!(post.advanced[base + 3], 5.0, "fx_fm_indice reached the mirror");
    }

    /// `fx.wavewarp` is an advanced group *and* a chain step, like `fx.fm`.
    #[test]
    fn a_fixed_wavewarp_node_is_a_chain_step_with_its_cant_as_the_amount() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(&mut g, &gen, "wavewarp", EffectState::Fixed(0.0), ControlSource::Row)
            .expect("materializes");
        let ww = metal_graph::effects::chain(&g, &gen)
            .into_iter()
            .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("wavewarp"))
            .expect("the wavewarp node");

        let mut post = Post::default();
        assert!(resolved_chain(&g, &gen, &mut post, &[]).is_empty(), "amount 0 spends no step");

        crate::bind::set_config_number(&mut g, &ww, "fx_wavewarp_cant", 0.7);
        crate::bind::set_config_number(&mut g, &ww, "fx_wavewarp_alto", 0.3);
        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);
        let kind = metal_schema::advanced_chain_kind("wavewarp").unwrap();
        assert_eq!(chain, vec![EffectStep { effect: kind, amount: 0.7, slot: 0 }]);
        let base = metal_schema::advanced_effect_span("wavewarp").unwrap().start - metal_schema::CHAIN_FLOATS;
        assert_eq!(post.advanced[base + 3], 0.3, "fx_wavewarp_alto reached the mirror");
    }

    /// `fx.ascii` is a chain step like `fx.fm`: `cant` is the amount, off
    /// spends no step, and its other floats reach `Post.advanced`.
    #[test]
    fn a_fixed_ascii_node_is_a_chain_step_with_its_cant_as_the_amount() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(&mut g, &gen, "ascii", EffectState::Fixed(0.0), ControlSource::Row)
            .expect("materializes");
        let node = metal_graph::effects::chain(&g, &gen)
            .into_iter()
            .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("ascii"))
            .expect("the ascii node");

        let mut post = Post::default();
        assert!(resolved_chain(&g, &gen, &mut post, &[]).is_empty(), "amount 0 spends no step");

        crate::bind::set_config_number(&mut g, &node, "fx_ascii_cant", 0.7);
        crate::bind::set_config_number(&mut g, &node, "fx_ascii_celda", 12.0);
        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);
        let kind = metal_schema::advanced_chain_kind("ascii").unwrap();
        assert_eq!(chain, vec![EffectStep { effect: kind, amount: 0.7, slot: 0 }]);
        let base = metal_schema::advanced_effect_span("ascii").unwrap().start - metal_schema::CHAIN_FLOATS;
        assert_eq!(post.advanced[base + 1], 12.0, "fx_ascii_celda reached the mirror");
    }

    /// `fx.brush` is a chain step like `fx.fm`: `cant` is the amount, off
    /// spends no step, and its other floats reach `Post.advanced`.
    #[test]
    fn a_fixed_brush_node_is_a_chain_step_with_its_cant_as_the_amount() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(&mut g, &gen, "brush", EffectState::Fixed(0.0), ControlSource::Row)
            .expect("materializes");
        let node = metal_graph::effects::chain(&g, &gen)
            .into_iter()
            .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("brush"))
            .expect("the brush node");

        let mut post = Post::default();
        assert!(resolved_chain(&g, &gen, &mut post, &[]).is_empty(), "amount 0 spends no step");

        crate::bind::set_config_number(&mut g, &node, "fx_brush_cant", 0.7);
        crate::bind::set_config_number(&mut g, &node, "fx_brush_n_trazos", 900.0);
        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);
        let kind = metal_schema::advanced_chain_kind("brush").unwrap();
        assert_eq!(chain, vec![EffectStep { effect: kind, amount: 0.7, slot: 0 }]);
        let base = metal_schema::advanced_effect_span("brush").unwrap().start - metal_schema::CHAIN_FLOATS;
        assert_eq!(post.advanced[base + 2], 900.0, "fx_brush_n_trazos reached the mirror");
    }

    /// `fx.hardstutter` is an advanced group *and* a chain step, like `fx.fm`.
    #[test]
    fn a_fixed_hardstutter_node_is_a_chain_step_with_its_cant_as_the_amount() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(&mut g, &gen, "hardstutter", EffectState::Fixed(0.0), ControlSource::Row)
            .expect("materializes");
        let hs = metal_graph::effects::chain(&g, &gen)
            .into_iter()
            .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("hardstutter"))
            .expect("the hardstutter node");

        let mut post = Post::default();
        assert!(resolved_chain(&g, &gen, &mut post, &[]).is_empty(), "amount 0 spends no step");

        crate::bind::set_config_number(&mut g, &hs, "fx_hardstutter_cant", 0.7);
        crate::bind::set_config_number(&mut g, &hs, "fx_hardstutter_duty", 0.3);
        let mut post = Post::default();
        let chain = resolved_chain(&g, &gen, &mut post, &[]);
        let kind = metal_schema::advanced_chain_kind("hardstutter").unwrap();
        assert_eq!(chain, vec![EffectStep { effect: kind, amount: 0.7, slot: 0 }]);
        let base = metal_schema::advanced_effect_span("hardstutter").unwrap().start - metal_schema::CHAIN_FLOATS;
        assert_eq!(post.advanced[base + 2], 0.3, "fx_hardstutter_duty reached the mirror");
    }

    /// A reactive-from-row advanced node must leave whatever `drive::row`
    /// computed in `post.advanced` exactly alone — the advanced mirror of the
    /// router-sourced rule for the common effects.
    #[test]
    fn a_reactive_advanced_node_keeps_the_rows_own_advanced_floats() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _out) = wire_gen_out(&mut g);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "grade",
            EffectState::Reactive,
            ControlSource::Row,
        )
        .expect("materializes");

        let mut post = Post::default();
        let span = metal_schema::advanced_effect_span("grade").unwrap();
        let base = span.start - metal_schema::CHAIN_FLOATS;
        post.advanced[base + 2] = 33.0; // what drive::row happened to compute
        let chain = resolved_chain(&g, &gen, &mut post, &[]);
        assert!(chain.is_empty());
        assert_eq!(post.advanced[base + 2], 33.0, "router-sourced must be untouched");
    }

    /// M4: the `mat.*` chain a generator carries is read back off the graph and
    /// baked into the row's `material` group by `chain_for_row` (rule 4).
    #[test]
    fn a_material_node_bakes_its_kind_into_the_rows_group() {
        use metal_graph::materials;
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.monster", [0.0, 0.0]);
        let mat = g.add_node("m", "mat.wood", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mat, "image"));

        let mut row = vec![0.0f32; metal_schema::EFFECTS_WIDTH];
        chain_for_row(&g, &gen, &mut row, &[]);
        let material = metal_schema::read_material(&row).expect("the row carries the group");
        assert_eq!(material[0], materials::kind_index("mat.wood").unwrap() as f32);
        assert_eq!(material[1], 1.0, "a material node turns the mix on");

        let resolved = resolved_materials(&g, &gen, &row);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind, "wood");
        assert_eq!(resolved[0].part, materials::PART_ANY);
    }
}
