//! Every renderer in the suite, as a declarative table.
//!
//! **The table is the source; everything else is derived.** `all_models()`,
//! the `metal-graph` catalog and the `emit-schema` manifest all read
//! [`MODEL_SPECS`], so none of them carries a list that can drift from the
//! others. `all_models().len()` is still the count — the number is
//! deliberately not written down here.
//!
//! One model in the suite is **not** in this table: **Don Joe** — the rig
//! danced — because he has no renderer. `metal-dash` draws him live, and lists
//! him greyed for the same reason. Don Joe is the rig in every sense.
//!
//! **The order is append-only.** `metal-dash`'s `/select` takes an index into
//! this table and an operator's on-air choice is that number; inserting a
//! model in the middle silently moves every animation after it. Ids are what
//! everything durable — presets, authored examples, a `/live` push — names a
//! model by, and those do not move. `docs/plan-architecture.md` LAY.14 adds a lookup
//! **by id** beside the index so a caller can stop depending on position; the
//! index path stays for compatibility.
//!
//! # Retiring a model: the tombstone
//!
//! Deleting a row is not allowed — it would slide every model after it onto
//! the wrong index, silently repainting saved layers and routed classes. So a
//! removed model leaves a **tombstone**: a [`Retired`] placeholder that keeps
//! the table position and the id (a saved reference still resolves to it) but
//! draws nothing and reports [`Model::retired`]. Derived surfaces — the
//! `data/schema.json` manifest, `tools/song-gate/models.json`, the graph
//! catalog, the demo previews — drop retired rows, so the app never offers a
//! row that draws a black frame.

use crate::model::{Block, Capabilities, Ctx, Model, Schema};

use crate::models::ascii::Ascii;
use crate::models::aurora::Aurora;
use crate::models::beam::Beam;
use crate::models::brasa::Brasa;
use crate::models::chamber::Chamber;
use crate::models::cyberspace::Cyberspace;
use crate::models::dla::Dla;
use crate::models::domain::Domain;
use crate::models::entrance::Entrance;
use crate::models::escena3d::Escena3d;
use crate::models::escape::Escape;
use crate::models::estrobo::Estrobo;
use crate::models::forge::Forge;
use crate::models::fractals::Fractals;
use crate::models::geoterrain::Geoterrain;
use crate::models::humo::Humo;
use crate::models::kaleido::Kaleido;
use crate::models::lazer::Lazer;
use crate::models::membrane::Membrane;
use crate::models::mengano::Mengano;
use crate::models::moire::Moire;
use crate::models::monstruo::Monstruo;
use crate::models::ondas::Ondas;
use crate::models::fineline::Fineline;
use crate::models::fluido::Fluido;
use crate::models::nubes::Nubes;
use crate::models::particulas::Particulas;
use crate::models::corales::Corales;
use crate::models::paisaje::Paisaje;
use crate::models::oracle::Oracle;
use crate::models::parallax::{ParallaxCampos, ParallaxCielo, ParallaxCiudad, ParallaxCueva};
use crate::models::trafico::Trafico;
use crate::models::warping::Warping;
use crate::models::objeto::Objeto;
use crate::models::catenoide::Catenoide;
use crate::models::bulbo::Bulbo;
use crate::models::colorpanels::Colorpanels;
use crate::models::dotgrid::Dotgrid;
use crate::models::heatmap::Heatmap;
use crate::models::spiral::Spiral;
use crate::models::meshgradient::Meshgradient;
use crate::models::dithering::Dithering;
use crate::models::dotorbit::Dotorbit;
use crate::models::gemsmoke::Gemsmoke;
use crate::models::flutedglass::Flutedglass;
use crate::models::godrays::Godrays;
use crate::models::graingradient::Graingradient;
use crate::models::halftonedots::Halftonedots;
use crate::models::metaballs::Metaballs;
use crate::models::halftonecmyk::Halftonecmyk;
use crate::models::liquidmetal::Liquidmetal;
use crate::models::imagedithering::Imagedithering;
use crate::models::neuronoise::Neuronoise;
use crate::models::sheettexture::Sheettexture;
use crate::models::perlinnoise::Perlinnoise;
use crate::models::simplexnoise::Simplexnoise;
use crate::models::smokering::Smokering;
use crate::models::pulsingborder::Pulsingborder;
use crate::models::swirl::Swirl;
use crate::models::voronoi::Voronoi;
use crate::models::warpcut::Warpcut;
use crate::models::ripples::Ripples;
use crate::models::water::Water;
use crate::models::puzzle::Puzzle;
use crate::models::relampago::Relampago;
use crate::models::rings::Rings;
use crate::models::rio::Rio;
use crate::models::room::Room;
use crate::models::sandstorm::Sandstorm;
use crate::models::scratch::Scratch;
use crate::models::solvent::Solvent;
use crate::models::terrario::Terrario;
use crate::models::turing::Turing;
use crate::models::vaporgrid::Vaporgrid;
use crate::models::vitral::Vitral;
use crate::models::isoflow::Isoflow;
use crate::models::ringflow::Ringflow;
use crate::models::caustic_weave::CausticWeave;
use crate::models::cellfield::Cellfield;
use crate::models::helix_thread::HelixThread;
use crate::models::chargefield::Chargefield;
use crate::models::form_cut::FormCut;
use crate::models::morphform::Morphform;
use crate::models::shatter_mask::ShatterMask;
use crate::models::hairlines::Hairlines;
use crate::models::glyph_stele::GlyphStele;
use crate::models::magnet_spikes::MagnetSpikes;
use crate::models::spore_drift::SporeDrift;
use crate::models::cymatics::Cymatics;
use crate::models::concrete::Concrete;
use crate::models::effigies::Effigies;
use crate::models::swarmfield::Swarmfield;
use crate::models::inkline::Inkline;
use crate::models::slumped_grid::SlumpedGrid;
use crate::models::blobfield::Blobfield;
use crate::models::chrome_pour::ChromePour;
use crate::models::phosphor_trace::PhosphorTrace;
use crate::models::spectral_ridges::SpectralRidges;
use crate::models::orbit_swarm::OrbitSwarm;
use crate::models::deep_tunnel::DeepTunnel;

/// One model in the suite: its stable id and how to build it.
///
/// Capacities and the `Schema` are read off the built model rather than stored
/// here — the model is the authority on what it accepts, and a second copy of
/// its width in a table is exactly the drift this table exists to remove.
pub struct ModelSpec {
    /// The stable id in `data/schema.json`, the folder an example lands in,
    /// and what a `/live` push names. ASCII, never changes for cosmetic
    /// reasons.
    pub id: &'static str,
    pub build: fn() -> Box<dyn Model>,
}

macro_rules! spec {
    ($id:literal, $ty:path) => {
        ModelSpec { id: $id, build: || Box::new(<$ty>::new()) }
    };
}

/// Retires a row: the id and the table position stay, nothing is built.
///
/// See the module doc. A tombstone keeps its id so a saved reference still
/// resolves (to a no-op) and so `index_of_id` does not move, but it reports
/// [`Model::retired`] and every derived surface drops it.
macro_rules! tombstone {
    ($id:literal) => {
        ModelSpec { id: $id, build: || Box::new(Retired { id: $id }) }
    };
}

/// The placeholder a retired model leaves in [`MODEL_SPECS`]. Draws nothing,
/// declares no capabilities, and is the only implementor of
/// [`Model::retired`] that answers `true`.
pub struct Retired {
    id: &'static str,
}

impl Model for Retired {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> &'static str {
        "Retired"
    }

    fn schema(&self) -> &'static Schema {
        static SCHEMA: Schema = Schema { blocks: &[] as &[Block] };
        &SCHEMA
    }

    fn render(
        &mut self,
        _ctx: &Ctx,
        _floats: &[f32],
        _target: &wgpu::TextureView,
        _format: wgpu::TextureFormat,
    ) {
        // A tombstone has nothing to draw; the target is left as the caller
        // cleared it. Deliberately not a debug panic — a saved layer may point
        // at this index and must not crash the show.
    }

    fn retired(&self) -> bool {
        true
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }
}

/// The suite, in the order it was built. **Append-only** — see the module doc.
pub const MODEL_SPECS: &[ModelSpec] = &[
    spec!("geoterrain", Geoterrain),
    spec!("domain", Domain),
    spec!("escape", Escape),
    spec!("entrance", Entrance),
    spec!("cyberspace", Cyberspace),
    spec!("solvent", Solvent),
    spec!("chamber", Chamber),
    spec!("membrane", Membrane),
    spec!("oracle", Oracle),
    // 2026-09-28: `pov` retired (the gaze rig). Tombstone, not deletion.
    tombstone!("pov"),
    spec!("turing", Turing),
    // The six asked for on 2026-08-17, appended in `docs/NEXT.md` order:
    // cheapest first, the one that needed a new capability (glyphs) after
    // those, and the rig last.
    spec!("fractals", Fractals),
    spec!("kaleido", Kaleido),
    spec!("vaporgrid", Vaporgrid),
    // 2026-10-16: `matrix` renamed to `room` (materials M6); the id becomes a
    // material, the model a room. Tombstone, not deletion (rule 1).
    tombstone!("matrix"),
    spec!("ascii", Ascii),
    // 2026-09-28: `walking` retired with its rig sibling `pov`.
    tombstone!("walking"),
    // The five asked for on 2026-08-22, in the order they were asked for.
    // Four are 2D objects with state, not fields over the frame.
    spec!("puzzle", Puzzle),
    spec!("someone", Mengano),
    spec!("rings", Rings),
    spec!("scratch", Scratch),
    spec!("laser", Lazer),
    // 2026-08-23. The first model with no CPU side at all: sand has no
    // decision in it, so the whole thing is the shader's inversion of the fall.
    spec!("sandstorm", Sandstorm),
    // The five asked for on 2026-08-23, after a ranking. Four share
    // `shared::fullscreen`; Lightning draws through `line2d`.
    spec!("strobe", Estrobo),
    spec!("lightning", Relampago),
    spec!("moire", Moire),
    spec!("stained-glass", Vitral),
    spec!("smoke", Humo),
    // 2026-08-23. Sandstorm's vocabulary poured into a box with a floor — the
    // second model that keeps state, because a pile is a history.
    spec!("terrarium", Terrario),
    // 2026-08-24. Matrix's grammar outdoors — the same weave, the same
    // one-light rule, the same single colour slot, on a river valley. A
    // separate model because a bush that answers the wind is a vertex that
    // moves, which Matrix's one invariant forbids.
    spec!("river", Rio),
    // 2026-08-25. The first **transmission**: a model that modifies a picture
    // instead of generating one. Same `Schema` contract as everything above,
    // which is why nothing downstream of `render` had to learn what footage
    // is. It draws its own slate when it has no library, and `Model::footage`
    // is what lets the app say why.
    // 2026-10-25: `wake` retired as a model (it was an effect). Tombstone, not
    // deletion (rule 1): the row keeps its index.
    tombstone!("wake"),
    // 2026-09-28: `quimica` retired (the colour-distorting filter).
    // Tombstone, not deletion — its row keeps the chain numbering so a saved
    // layer does not renumber.
    tombstone!("quimica"),
    // 2026-08-25. Two picks ported from fosfora: spectrogram curtain bands and
    // a vector-CRT trace. Both generative, so plain at the trait's common end.
    spec!("aurora", Aurora),
    spec!("beam", Beam),
    // 2026-09-05. `the design notes` §H.1 — the template the AI harness
    // reads as its starting point for a personalised shader.
    spec!("scene3d", Escena3d),
    // 2026-09-14. Diffusion-limited aggregation — the walker-ring algorithm as
    // a model. The second model that keeps state, and the one that could not
    // not: an aggregate is a history.
    spec!("dla", Dla),
    // 2026-09-25. The model whose shader is written at runtime — by the AI,
    // through `save_shader` — and validated by `metal_forge::check` before it
    // draws. Its row is the sixteen ingest columns as they arrive; which WGSL
    // it draws is the layer's own `shader`.
    spec!("forge", Forge),
    spec!("monster", Monstruo),
    // 2026-10-07. Parallax worlds: one engine (`shared::parallax`), four
    // entries so the router can pick a world. Gate-equivalent to `terrarium`.
    spec!("parallax-cave", ParallaxCueva),
    spec!("parallax-sky", ParallaxCielo),
    spec!("parallax-fields", ParallaxCampos),
    spec!("parallax-city", ParallaxCiudad),
    // 2026-10-15. Wave 2: Biham-Middleton-Levine traffic automaton.
    spec!("traffic", Trafico),
    // 2026-10-15. Doom-fire cellular automaton on a heat grid.
    spec!("ember", Brasa),
    // 2026-10-15. Hydra-style chained transform synth with feedback.
    spec!("warping", Warping),
    // 2026-10-15. Gielis superformula object, ray-marched.
    spec!("object", Objeto),
    // 2026-10-15. Ray-cast catenoid tunnel room.
    spec!("catenoid", Catenoide),
    // 2026-10-15. 2D FDTD electromagnetic field (TMz Yee scheme), after Meep.
    spec!("waves", Ondas),
    // 2026-10-15. GPU Navier-Stokes liquid colour, after WebGL-Fluid-Simulation.
    spec!("fluid", Fluido),
    // 2026-10-15. 2D compressible Euler flow (Kelvin-Helmholtz) drawn as clouds.
    spec!("clouds", Nubes),
    // 2026-10-15. N-body particle system (leapfrog, shearing sheet), after rebound.
    spec!("particles", Particulas),
    // 2026-10-15. Differential line growth, after inconvergent/differential-line.
    spec!("corals", Corales),
    // 2026-10-15. 3D fine-line plotter drawing, after abey79/lines + vpype.
    spec!("fineline", Fineline),
    // 2026-10-15. Scrolling ink landscape, after LingDong-/shan-shui-inf.
    spec!("landscape", Paisaje),
    // 2026-10-15. Ray-marched 3D fractals (8 curated estimators), after mandelbulber2.
    spec!("bulb", Bulbo),
    // 2026-10-16. Procedural model: ring of translucent glass panels.
    spec!("colorpanels", Colorpanels),
    // 2026-10-16. Procedural model: lattice of shapes.
    spec!("dotgrid", Dotgrid),
    // 2026-10-16. Procedural model: ordered/random dithered procedural field.
    spec!("dithering", Dithering),
    // 2026-10-16. Procedural model: dots circling their cells.
    spec!("dotorbit", Dotorbit),
    // 2026-10-16. Procedural model: glassy gem with smoke inside and around.
    spec!("gemsmoke", Gemsmoke),
    // 2026-10-16. Procedural model: ribbed glass over a source picture.
    spec!("flutedglass", Flutedglass),
    // 2026-10-16. Procedural model: light shafts with core glow and bloom.
    spec!("godrays", Godrays),
    // 2026-10-16. Procedural model: grain gradient + static radial gradient.
    spec!("graingradient", Graingradient),
    // 2026-10-16. Procedural model: picture as a halftone dot screen.
    spec!("halftonedots", Halftonedots),
    // 2026-10-16. Procedural model: picture reprinted as CMYK halftone screens.
    spec!("halftonecmyk", Halftonecmyk),
    // 2026-10-16. Procedural model: source shape as a thermal map with halo and core.
    spec!("heatmap", Heatmap),
    // 2026-10-16. Procedural model: chrome liquid metal over a source shape.
    spec!("liquidmetal", Liquidmetal),
    // 2026-10-16. Procedural model: source picture through a threshold map.
    spec!("imagedithering", Imagedithering),
    // 2026-10-16. Procedural model: orbiting liquid blobs that merge.
    spec!("metaballs", Metaballs),
    // 2026-10-16. Procedural model: colour-point mesh gradient + static mesh gradient.
    spec!("meshgradient", Meshgradient),
    // 2026-10-16. Procedural model: glowing web of neural filaments.
    spec!("neuronoise", Neuronoise),
    // 2026-10-16. Procedural model: lit paper sheet (fibres, crumples, folds, drops) over a source.
    spec!("sheettexture", Sheettexture),
    // 2026-10-16. Procedural model: fractal gradient-noise field cut at a threshold.
    spec!("perlinnoise", Perlinnoise),
    // 2026-10-16. Procedural model: stepped palette on 3D simplex noise.
    spec!("simplexnoise", Simplexnoise),
    // 2026-10-16. Procedural model: ring of turbulent smoke around a hollow centre.
    spec!("smokering", Smokering),
    // 2026-10-16. Procedural model: glowing rounded frame with orbiting spots, smoke and bloom.
    spec!("pulsingborder", Pulsingborder),
    // 2026-10-16. Procedural model: multi-arm spiral strokes, Archimedean to log.
    spec!("spiral", Spiral),
    // 2026-10-16. Procedural model: swirling vortex of colour bands.
    spec!("swirl", Swirl),
    // 2026-10-16. Procedural model: jittered cellular mosaic, stepped colours, gap lines and glow.
    spec!("voronoi", Voronoi),
    // 2026-10-16. Procedural model: iterated-swirl warped checks/stripes/edge/rings in four colours.
    spec!("warpcut", Warpcut),
    // 2026-10-16. Procedural model: rows of undulating lines, sine to noise, braided and dashed.
    spec!("ripples", Ripples),
    // 2026-10-16. Procedural model: picture refracted through caustic, rippling water.
    spec!("water", Water),
    // 2026-10-16. Materials M6: `matrix`'s room, under the name the material
    // took, as an ordinary 3D model that draws filled faces with real normals.
    spec!("room", Room),
    // 2026-10-21. Ported community tool "Isoflow" as a procedural model.
    spec!("isoflow", Isoflow),
    // 2026-10-21. Ported community tool "Ringflow" as a procedural model.
    spec!("ringflow", Ringflow),
    // 2026-10-21. Ported community tool "CausticWeave" as a procedural model.
    spec!("caustic-weave", CausticWeave),
    // 2026-10-21. Ported community tool "Cellfield" as a procedural model.
    spec!("cellfield", Cellfield),
    // 2026-10-21. Ported community tool "HelixThread" as a procedural model.
    spec!("helix-thread", HelixThread),
    // 2026-10-21. Ported community tool "Chargefield" as a procedural model.
    spec!("chargefield", Chargefield),
    // 2026-10-21. Ported community tool "FormCut" as a procedural model.
    spec!("form-cut", FormCut),
    // 2026-10-21. Ported community tool "Morphform" as a procedural model.
    spec!("morphform", Morphform),
    // 2026-10-21. Ported community tool "ShatterMask" as a procedural model.
    spec!("shatter-mask", ShatterMask),
    // 2026-10-21. Ported community tool "Hairlines" as a procedural model.
    spec!("hairlines", Hairlines),
    // 2026-10-21. Ported community tool "GlyphStele" as a procedural model.
    spec!("glyph-stele", GlyphStele),
    // 2026-10-21. Ported built-in tool "magnet-spikes" as a procedural model.
    spec!("magnet-spikes", MagnetSpikes),
    // 2026-10-21. Ported built-in tool "spore-drift" as a procedural model.
    spec!("spore-drift", SporeDrift),
    // 2026-10-21. Ported built-in tool "cymatics" as a procedural model.
    spec!("cymatics", Cymatics),
    // 2026-10-21. Ported built-in tool "concrete" as a procedural model.
    spec!("concrete", Concrete),
    // 2026-10-21. Ported built-in tool "effigies" as a procedural model.
    spec!("effigies", Effigies),
    // 2026-10-21. Ported built-in tool "swarmfield" as a procedural model.
    spec!("swarmfield", Swarmfield),
    // 2026-10-21. Ported built-in tool "inkline" as a procedural model.
    spec!("inkline", Inkline),
    // 2026-10-21. Ported built-in tool "slumped-grid" as a procedural model.
    spec!("slumped-grid", SlumpedGrid),
    // 2026-10-21. Ported built-in tool "blobfield" (equivalent was not the same).
    spec!("blobfield", Blobfield),
    // 2026-10-21. Ported built-in tool "chrome-pour" (equivalent was not the same).
    spec!("chrome-pour", ChromePour),
    // 2026-10-21. Ported built-in tool "phosphor-trace" (equivalent was not the same).
    spec!("phosphor-trace", PhosphorTrace),
    // 2026-10-21. Ported built-in tool "spectral-ridges" (equivalent was not the same).
    spec!("spectral-ridges", SpectralRidges),
    // 2026-10-21. Ported built-in tool "orbit-swarm" (equivalent was not the same).
    spec!("orbit-swarm", OrbitSwarm),
    // 2026-10-21. Ported built-in tool "deep-tunnel" (equivalent was not the same).
    spec!("deep-tunnel", DeepTunnel),
];

/// Every renderer in the suite, built fresh, in table order.
pub fn all_models() -> Vec<Box<dyn Model>> {
    MODEL_SPECS.iter().map(|s| (s.build)()).collect()
}

/// Old (Spanish) model ids still accepted by [`model_by_id`] and
/// [`index_of_id`]: presets, graphs and device files written before the
/// English-ids migration name a model by its Spanish id. Each maps to the
/// English id it was renamed to. Do not add new entries; new ids are English.
pub const MODEL_ID_ALIASES: &[(&str, &str)] = &[
    ("brasa", "ember"),
    ("bulbo", "bulb"),
    ("catenoide", "catenoid"),
    ("corales", "corals"),
    ("escena3d", "scene3d"),
    ("estela", "wake"),
    ("estrobo", "strobe"),
    ("fluido", "fluid"),
    ("humo", "smoke"),
    ("lazer", "laser"),
    ("mengano", "someone"),
    ("monstruo", "monster"),
    ("nubes", "clouds"),
    ("objeto", "object"),
    ("ondas", "waves"),
    ("paisaje", "landscape"),
    ("parallax-campos", "parallax-fields"),
    ("parallax-cielo", "parallax-sky"),
    ("parallax-ciudad", "parallax-city"),
    ("parallax-cueva", "parallax-cave"),
    ("particulas", "particles"),
    ("relampago", "lightning"),
    ("rio", "river"),
    ("terrario", "terrarium"),
    ("trafico", "traffic"),
    ("vitral", "stained-glass"),
];

/// The current id for `id`: an English id is returned unchanged, a Spanish
/// alias from [`MODEL_ID_ALIASES`] is mapped to its English id.
pub fn canonical_model_id(id: &str) -> &str {
    MODEL_ID_ALIASES.iter().find(|(old, _)| *old == id).map_or(id, |(_, new)| *new)
}

/// Builds a model **by id**, independent of its position in the table.
///
/// This is the lookup `docs/plan-architecture.md` LAY.14 adds: an operator's saved
/// preset names an id, and inserting a new model in the middle of
/// [`MODEL_SPECS`] must not silently point that preset at a different model.
/// The index path stays in `metal-dash` for the shell's on-air choice; new code
/// should prefer this.
pub fn model_by_id(id: &str) -> Option<Box<dyn Model>> {
    let id = canonical_model_id(id);
    MODEL_SPECS.iter().find(|s| s.id == id).map(|s| (s.build)())
}

/// The table position of an id, or `None`. The bridge between a saved id and
/// a positional surface (`/select`) without that surface having to keep its
/// own copy of the list.
pub fn index_of_id(id: &str) -> Option<usize> {
    let id = canonical_model_id(id);
    MODEL_SPECS.iter().position(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spanish_id_still_loads_the_english_model() {
        assert_eq!(model_by_id("humo").expect("old id loads").id(), "smoke");
        assert_eq!(index_of_id("humo"), index_of_id("smoke"));
        assert_eq!(canonical_model_id("rio"), "river");
        assert_eq!(canonical_model_id("rio"), "river");
    }

    #[test]
    fn every_spec_id_matches_the_model_it_builds() {
        for spec in MODEL_SPECS {
            let m = (spec.build)();
            assert_eq!(spec.id, m.id(), "the table's id and the model's id disagree");
        }
    }

    #[test]
    fn ids_are_unique_and_the_index_lookup_round_trips() {
        for (i, spec) in MODEL_SPECS.iter().enumerate() {
            assert_eq!(index_of_id(spec.id), Some(i), "{} does not round-trip", spec.id);
            assert!(model_by_id(spec.id).is_some(), "{} is not addressable by id", spec.id);
        }
        let mut sorted: Vec<&str> = MODEL_SPECS.iter().map(|s| s.id).collect();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "two models share an id");
        assert!(model_by_id("not-a-model").is_none());
    }

    #[test]
    fn all_models_follow_the_table_order() {
        let models = all_models();
        assert_eq!(models.len(), MODEL_SPECS.len());
        for (m, s) in models.iter().zip(MODEL_SPECS) {
            assert_eq!(m.id(), s.id);
        }
    }

    /// Models that expose a part but do not yet draw a material on it (the
    /// surface picture only). Each is a known gap; a new one fails the test
    /// until it is listed here on purpose, and a fixed one must leave it.
    const MATERIAL_GAPS: &[&str] = &[
        "corals",
        "water",
    ];

    /// Every model that exposes a part a material can be laid on takes a
    /// material on each part: at `mat_mix` 1 the frame changes, and clearing
    /// the part restores the unpainted frame byte for byte. The list is
    /// computed from `all_models()`, not written out, so a new surfaced model
    /// is covered the day it is registered.
    #[test]
    fn every_surfaced_model_takes_and_clears_a_material_on_each_part() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (96u32, 96u32);
        let mut covered = Vec::new();
        let mut gaps = Vec::new();
        for mut model in all_models() {
            let parts = model.surfaces();
            if parts.is_empty() {
                continue;
            }
            let floats = model.schema().defaults();
            let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
            let draw = |model: &mut dyn Model| {
                let ctx = Ctx {
                    device: &gpu.device,
                    queue: &gpu.queue,
                    time: 1.0,
                    width: w,
                    height: h,
                    fondo_alpha: false,
                    detail: 1.0,
                };
                model.render(&ctx, &floats, &view, FORMAT);
                crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
            };
            let id = model.id();
            let plain = draw(&mut *model);
            let mut ok = true;
            for part in parts {
                let material = || SurfaceSpec {
                    image: None,
                    material: Some(MaterialSpec { kind: 0.0, mix: 1.0, scale: 1.0, ..Default::default() }),
                };
                model.set_surface(part, Some(material()));
                let painted = draw(&mut *model);
                ok &= plain != painted;
                model.set_surface(part, None);
                let restored = draw(&mut *model);
                ok &= plain == restored;
            }
            if ok {
                covered.push(id);
            } else {
                gaps.push(id);
            }
        }
        assert!(!covered.is_empty(), "no model exposes a surface");
        // `geoterrain` draws through `line2d`, whose strokes wear the material.
        assert!(covered.contains(&"geoterrain"), "geoterrain's strokes take a material");
        assert_eq!(gaps, MATERIAL_GAPS, "models with a surface but no working material (covered: {covered:?})");
    }
}
