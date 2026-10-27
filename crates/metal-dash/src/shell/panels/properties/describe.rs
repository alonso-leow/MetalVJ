//! The properties card's per-kind text: one specific sentence and three
//! keypoints for every catalogued node kind, so the card says what *this*
//! node does and not only which family it belongs to. [`lookup`] is the one
//! reader; `describe_kind` in `mod.rs` falls back to its family text when a
//! kind has no entry here.
//!
//! Every string rides through `tr_key` so the i18n gates see it; the Spanish
//! of each lives in `locales/es/describe.toml`.

use crate::shell::i18n::tr_key;

/// One kind's card text.
pub(super) struct Entry {
    /// The catalogue kind: `gen.turing`, `fx.grade`, `audio.file`…
    pub kind: &'static str,
    /// One sentence (about 100 characters at most) saying what this specific
    /// node does.
    pub description: &'static str,
    /// Three short facts (one to three words): its main parameters or ports.
    pub keypoints: [&'static str; 3],
}

/// The entry for `kind`, exact match only.
pub(super) fn lookup(kind: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.kind == kind)
}

/// Every kind with text of its own, grouped by family in the order the
/// catalogue lists them.
const ENTRIES: &[Entry] = &[
    // ---- generator ----
    Entry {
        kind: "gen.geoterrain",
        description: tr_key("Parametric spiral arms that twist out from the centre, drawn as thick lines."),
        keypoints: [tr_key("arm count"), tr_key("twist"), tr_key("thickness")],
    },
    Entry {
        kind: "gen.domain",
        description: tr_key("Scanlines, row tearing and RGB split that make the frame look like a broken signal."),
        keypoints: [tr_key("scanlines"), tr_key("distortion"), tr_key("rgb split")],
    },
    Entry {
        kind: "gen.escape",
        description: tr_key("Endless strings that cross the frame and sway, drawn as thin lines."),
        keypoints: [tr_key("string count"), tr_key("tension"), tr_key("thickness")],
    },
    Entry {
        kind: "gen.entrance",
        description: tr_key("Instanced billboards streaming toward a camera that keeps advancing into depth."),
        keypoints: [tr_key("advance speed"), tr_key("density"), tr_key("palette variation")],
    },
    Entry {
        kind: "gen.cyberspace",
        description: tr_key("Translucent parallax layers of neon panels with bloom, drifting in depth."),
        keypoints: [tr_key("layer count"), tr_key("depth"), tr_key("parallax speed")],
    },
    Entry {
        kind: "gen.solvent",
        description: tr_key("Soft drops that blur and merge into two liquids, grained like film."),
        keypoints: [tr_key("drop count"), tr_key("viscosity"), tr_key("agitation")],
    },
    Entry {
        kind: "gen.chamber",
        description: tr_key("A 3D grid of cubes whose height pulses outward from a point under gravity."),
        keypoints: [tr_key("pulse height"), tr_key("propagation"), tr_key("gravity")],
    },
    Entry {
        kind: "gen.membrane",
        description: tr_key("Dense hairs swept by a noise wind, growing from a skin-like surface."),
        keypoints: [tr_key("hair length"), tr_key("density"), tr_key("wind strength")],
    },
    Entry {
        kind: "gen.oracle",
        description: tr_key("An eye, or several, whose iris and lids dart to the frame in quick glances."),
        keypoints: [tr_key("gaze"), tr_key("eyelid"), tr_key("pupil")],
    },
    Entry {
        kind: "gen.turing",
        description: tr_key("Reaction-diffusion patterns that grow like animal skin."),
        keypoints: [tr_key("feed rate"), tr_key("kill rate"), tr_key("palette")],
    },
    Entry {
        kind: "gen.fractals",
        description: tr_key("A Julia set coloured by how fast each point escapes, zooming and drifting slowly."),
        keypoints: [tr_key("zoom"), tr_key("julia constant"), tr_key("detail")],
    },
    Entry {
        kind: "gen.kaleido",
        description: tr_key("Spiral arms folded into a mirrored wedge that turns like a kaleidoscope."),
        keypoints: [tr_key("mirrors"), tr_key("arms"), tr_key("twist")],
    },
    Entry {
        kind: "gen.vaporgrid",
        description: tr_key("A receding grid under a sun, scrolling forever toward the horizon."),
        keypoints: [tr_key("scroll speed"), tr_key("grid lines"), tr_key("glitch")],
    },
    Entry {
        kind: "gen.matrix",
        description: tr_key("A wireframe room with furniture, drawn in grid lines and lit rather than moved."),
        keypoints: [tr_key("brightness"), tr_key("pulse"), tr_key("sweep")],
    },
    Entry {
        kind: "gen.ascii",
        description: tr_key("A waveform redrawn in text characters, one glyph per grid cell."),
        keypoints: [tr_key("amplitude"), tr_key("columns"), tr_key("speed")],
    },
    Entry {
        kind: "gen.puzzle",
        description: tr_key("A tile board that changes only when pieces slide in from its four edges."),
        keypoints: [tr_key("columns"), tr_key("rows"), tr_key("slide")],
    },
    Entry {
        kind: "gen.someone",
        description: tr_key("A squishy ball with eyes that crosses the frame or stands while the frame passes."),
        keypoints: [tr_key("elasticity"), tr_key("background speed"), tr_key("blink")],
    },
    Entry {
        kind: "gen.rings",
        description: tr_key("A ball bouncing inside turning rings, each with a gap it can escape through."),
        keypoints: [tr_key("ring count"), tr_key("speed"), tr_key("gravity")],
    },
    Entry {
        kind: "gen.scratch",
        description: tr_key("A coating scratched away wherever things are dragged across it, until all is gone."),
        keypoints: [tr_key("items"), tr_key("angle"), tr_key("grain")],
    },
    Entry {
        kind: "gen.laser",
        description: tr_key("Beams bouncing off the frame edges and any figures, leaving bright trails behind."),
        keypoints: [tr_key("beam count"), tr_key("bounces"), tr_key("trail")],
    },
    Entry {
        kind: "gen.sandstorm",
        description: tr_key("Grains of sand pouring from a placed mouth and drifting on the wind as they fall."),
        keypoints: [tr_key("density"), tr_key("wind"), tr_key("grain size")],
    },
    Entry {
        kind: "gen.strobe",
        description: tr_key("The whole frame as light: beat-locked flashes with set duty, decay and band chases."),
        keypoints: [tr_key("period"), tr_key("duty"), tr_key("decay")],
    },
    Entry {
        kind: "gen.lightning",
        description: tr_key("Arcs jumping outward from one live centre to the edge of its reach, then fading."),
        keypoints: [tr_key("arc count"), tr_key("reach"), tr_key("branches")],
    },
    Entry {
        kind: "gen.moire",
        description: tr_key("Line rulings laid over each other, so the picture is the difference between them."),
        keypoints: [tr_key("layers"), tr_key("pitch"), tr_key("rotation")],
    },
    Entry {
        kind: "gen.stained-glass",
        description: tr_key("A leaded window of glass panes that drift and light up a few at a time."),
        keypoints: [tr_key("pane count"), tr_key("lead"), tr_key("burst")],
    },
    Entry {
        kind: "gen.smoke",
        description: tr_key("Smoke rising from a source, curled by a warp and lit from one direction."),
        keypoints: [tr_key("rise"), tr_key("density"), tr_key("light direction")],
    },
    Entry {
        kind: "gen.terrarium",
        description: tr_key("A glass box filled from above by coloured sand that piles into the picture."),
        keypoints: [tr_key("sources"), tr_key("flow"), tr_key("vibration")],
    },
    Entry {
        kind: "gen.river",
        description: tr_key("A river running away between banks, with stones in the water and bushes that sway."),
        keypoints: [tr_key("current"), tr_key("wind"), tr_key("brightness")],
    },
    Entry {
        kind: "gen.aurora",
        description: tr_key("Horizontal curtains of light, one per frequency band, moving like a spectrogram."),
        keypoints: [tr_key("speed"), tr_key("band spacing"), tr_key("emphasis")],
    },
    Entry {
        kind: "gen.beam",
        description: tr_key("A vector oscilloscope trace that draws the music as a glowing line, like a CRT beam."),
        keypoints: [tr_key("decay"), tr_key("intensity"), tr_key("amplitude")],
    },
    Entry {
        kind: "gen.scene3d",
        description: tr_key("A raymarched box repeated across a tile grid, with metal and roughness controls."),
        keypoints: [tr_key("grid columns"), tr_key("rotation"), tr_key("metal")],
    },
    Entry {
        kind: "gen.dla",
        description: tr_key("Random walkers that stick where they touch, growing a branching coral-like cluster."),
        keypoints: [tr_key("walkers"), tr_key("stride"), tr_key("spark")],
    },
    Entry {
        kind: "gen.forge",
        description: tr_key("A custom WGSL shader loaded at runtime, fed by the audio level, onset and chroma."),
        keypoints: [tr_key("rms"), tr_key("onset"), tr_key("centroid")],
    },
    Entry {
        kind: "gen.monster",
        description: tr_key("A mythical monster face that sings, raps, whispers and talks, posed from the voice."),
        keypoints: [tr_key("species"), tr_key("eyes"), tr_key("aura")],
    },
    Entry {
        kind: "gen.parallax-cave",
        description: tr_key("Endless parallax of cave stickers on five depth planes, scrolling past the camera."),
        keypoints: [tr_key("direction"), tr_key("speed"), tr_key("density")],
    },
    Entry {
        kind: "gen.parallax-sky",
        description: tr_key("Endless parallax of sky stickers on five depth planes, scrolling past the camera."),
        keypoints: [tr_key("direction"), tr_key("speed"), tr_key("density")],
    },
    Entry {
        kind: "gen.parallax-fields",
        description: tr_key("Endless parallax of field stickers on five depth planes, scrolling past the camera."),
        keypoints: [tr_key("direction"), tr_key("speed"), tr_key("density")],
    },
    Entry {
        kind: "gen.parallax-city",
        description: tr_key("Endless parallax of city stickers on five depth planes, scrolling past the camera."),
        keypoints: [tr_key("direction"), tr_key("speed"), tr_key("density")],
    },
    Entry {
        kind: "gen.traffic",
        description: tr_key("Red and blue cars jamming a grid, following the Biham-Middleton-Levine traffic rule."),
        keypoints: [tr_key("density"), tr_key("red share"), tr_key("step speed")],
    },
    Entry {
        kind: "gen.ember",
        description: tr_key("Flames climbing a grid from a hot source row, cooling and bending with the wind."),
        keypoints: [tr_key("cooling"), tr_key("wind"), tr_key("embers")],
    },
    Entry {
        kind: "gen.warping",
        description: tr_key("A chained transform synth that warps, zooms and colours the frame in a fixed order."),
        keypoints: [tr_key("zoom"), tr_key("pan"), tr_key("rotation")],
    },
    Entry {
        kind: "gen.object",
        description: tr_key("A superformula solid, twisted and stretched, with rim light and a textured surface."),
        keypoints: [tr_key("torsion"), tr_key("stretch"), tr_key("texture")],
    },
    Entry {
        kind: "gen.catenoid",
        description: tr_key("A ray-cast tunnel whose wall follows a catenoid, with neon edges and rolling lights."),
        keypoints: [tr_key("neck radius"), tr_key("advance"), tr_key("spiral")],
    },
    Entry {
        kind: "gen.waves",
        description: tr_key("A 2D electromagnetic wave field on a grid, with sources, obstacles and contour lines."),
        keypoints: [tr_key("frequency"), tr_key("sources"), tr_key("gain")],
    },
    Entry {
        kind: "gen.fluid",
        description: tr_key("Swirling ink in a stable-fluids simulation, splatted to the beat and shaded with a sheen."),
        keypoints: [tr_key("vorticity"), tr_key("splat force"), tr_key("dissipation")],
    },
    Entry {
        kind: "gen.clouds",
        description: tr_key("Clouds from a compressible flow simulation, rolling into shear swirls and shadowed."),
        keypoints: [tr_key("shear"), tr_key("viscosity"), tr_key("shadow")],
    },
    Entry {
        kind: "gen.particles",
        description: tr_key("N-body particles that orbit a central mass and attractors under gravity."),
        keypoints: [tr_key("particle count"), tr_key("central mass"), tr_key("chaos")],
    },
    Entry {
        kind: "gen.corals",
        description: tr_key("A polyline that grows by differential line rules, splitting and repelling like coral."),
        keypoints: [tr_key("growth speed"), tr_key("split chance"), tr_key("stroke width")],
    },
    Entry {
        kind: "gen.fineline",
        description: tr_key("A 3D scene drawn as fine pen lines, hatched and stripped to silhouettes like a plotter."),
        keypoints: [tr_key("hatch density"), tr_key("stroke width"), tr_key("object count")],
    },
    Entry {
        kind: "gen.landscape",
        description: tr_key("An ink-wash landscape of mountains, trees and boats that scrolls like a scroll painting."),
        keypoints: [tr_key("scroll speed"), tr_key("mountain height"), tr_key("layers")],
    },
    Entry {
        kind: "gen.bulb",
        description: tr_key("A ray-marched 3D fractal from one of eight distance estimators, with orbit-trap colour."),
        keypoints: [tr_key("power"), tr_key("iterations"), tr_key("julia mix")],
    },
    Entry {
        kind: "gen.ramp",
        description: tr_key("A gradient between colour stops, shaped by angle and centre and repeated across the frame."),
        keypoints: [tr_key("shape"), tr_key("angle"), tr_key("repeat")],
    },
    Entry {
        kind: "gen.colorpanels",
        description: tr_key("A ring of translucent glass panels turning around a vertical axis."),
        keypoints: [tr_key("density"), tr_key("speed"), tr_key("perspective")],
    },
    Entry {
        kind: "gen.dotgrid",
        description: tr_key("A grid of dots, each sized and faded by a wave that ripples across the lattice."),
        keypoints: [tr_key("dot size"), tr_key("wave"), tr_key("roundness")],
    },
    Entry {
        kind: "gen.dithering",
        description: tr_key("A procedural field quantised to a few ink levels through a threshold pattern."),
        keypoints: [tr_key("levels"), tr_key("threshold"), tr_key("speed")],
    },
    Entry {
        kind: "gen.dotorbit",
        description: tr_key("Dots circling their own cells, each with a trail and a pulse as it orbits."),
        keypoints: [tr_key("dot count"), tr_key("speed"), tr_key("trail")],
    },
    Entry {
        kind: "gen.gemsmoke",
        description: tr_key("A glassy faceted gem with smoke trapped inside and drifting around it."),
        keypoints: [tr_key("facets"), tr_key("refraction"), tr_key("smoke speed")],
    },
    Entry {
        kind: "gen.flutedglass",
        description: tr_key("A picture seen through ribbed glass, with light and shadow bands sliding across."),
        keypoints: [tr_key("flute size"), tr_key("wave"), tr_key("chromatic")],
    },
    Entry {
        kind: "gen.godrays",
        description: tr_key("Light shafts fanning from a point, with a core glow and a bloom halo."),
        keypoints: [tr_key("ray count"), tr_key("bloom"), tr_key("density")],
    },
    Entry {
        kind: "gen.graingradient",
        description: tr_key("A grainy colour gradient whose bands and noise drift slowly across the frame."),
        keypoints: [tr_key("softness"), tr_key("grain"), tr_key("speed")],
    },
    Entry {
        kind: "gen.halftonecmyk",
        description: tr_key("A picture reprinted as four overprinted CMYK halftone screens."),
        keypoints: [tr_key("inks"), tr_key("paper"), tr_key("image input")],
    },
    Entry {
        kind: "gen.halftonedots",
        description: tr_key("A picture as a halftone dot screen, with dots that grow where the picture is dark."),
        keypoints: [tr_key("dot size"), tr_key("angle"), tr_key("threshold")],
    },
    Entry {
        kind: "gen.heatmap",
        description: tr_key("A source shape rendered as a thermal map, from a hot core to a cooler halo."),
        keypoints: [tr_key("threshold"), tr_key("glow"), tr_key("noise")],
    },
    Entry {
        kind: "gen.liquidmetal",
        description: tr_key("Chrome-like liquid metal poured over a shape, with a glossy fresnel rim."),
        keypoints: [tr_key("repeat"), tr_key("smoothness"), tr_key("fresnel")],
    },
    Entry {
        kind: "gen.imagedithering",
        description: tr_key("The source picture quantised to a few levels through a threshold map, in ink."),
        keypoints: [tr_key("threshold"), tr_key("colour steps"), tr_key("diffusion")],
    },
    Entry {
        kind: "gen.metaballs",
        description: tr_key("Soft blobs that orbit and merge like liquid, glowing where they meet."),
        keypoints: [tr_key("blob count"), tr_key("viscosity"), tr_key("attraction")],
    },
    Entry {
        kind: "gen.meshgradient",
        description: tr_key("A mesh of colour points blended into one smooth gradient that can be held still."),
        keypoints: [tr_key("colours"), tr_key("tint"), tr_key("zoom")],
    },
    Entry {
        kind: "gen.neuronoise",
        description: tr_key("A glowing web of neural filaments that branches and pulses across the frame."),
        keypoints: [tr_key("density"), tr_key("node pulse"), tr_key("sparks")],
    },
    Entry {
        kind: "gen.sheettexture",
        description: tr_key("A lit sheet of paper with fibres, folds, crumples and water drops over a picture."),
        keypoints: [tr_key("fibres"), tr_key("folds"), tr_key("ageing")],
    },
    Entry {
        kind: "gen.perlinnoise",
        description: tr_key("A field cut from fractal gradient noise, its contours shaped by a flowing warp."),
        keypoints: [tr_key("octaves"), tr_key("scale"), tr_key("bands")],
    },
    Entry {
        kind: "gen.pulsingborder",
        description: tr_key("A glowing rounded frame whose border carries orbiting colour spots and a pulse."),
        keypoints: [tr_key("border"), tr_key("smoke"), tr_key("halo")],
    },
    Entry {
        kind: "gen.simplexnoise",
        description: tr_key("A palette stepped onto 3D simplex noise, banded into flat colour steps."),
        keypoints: [tr_key("colour steps"), tr_key("octaves"), tr_key("hardness")],
    },
    Entry {
        kind: "gen.smokering",
        description: tr_key("A ring of turbulent smoke wrapped around a hollow centre with a bright core."),
        keypoints: [tr_key("radius"), tr_key("thickness"), tr_key("filaments")],
    },
    Entry {
        kind: "gen.spiral",
        description: tr_key("Multi-arm spiral strokes that wind from Archimedean to logarithmic, breathing."),
        keypoints: [tr_key("arms"), tr_key("stroke"), tr_key("density")],
    },
    Entry {
        kind: "gen.swirl",
        description: tr_key("A vortex of colour bands twisting into a centre, with an optional twin swirl."),
        keypoints: [tr_key("bands"), tr_key("twist"), tr_key("twin")],
    },
    Entry {
        kind: "gen.voronoi",
        description: tr_key("A stepped-colour cellular mosaic with distorted cells, gaps and a glow."),
        keypoints: [tr_key("cell scale"), tr_key("jitter"), tr_key("glow")],
    },
    Entry {
        kind: "gen.warpcut",
        description: tr_key("Checks, stripes or rings bent by an iterated swirl and noise, cut into four colours."),
        keypoints: [tr_key("shape"), tr_key("swirl"), tr_key("distortion")],
    },
    Entry {
        kind: "gen.ripples",
        description: tr_key("Rows of undulating lines that ripple across the frame, with a shaded band."),
        keypoints: [tr_key("frequency"), tr_key("amplitude"), tr_key("spacing")],
    },
    Entry {
        kind: "gen.water",
        description: tr_key("A picture seen through caustic, rippling water with reflections and foam."),
        keypoints: [tr_key("caustics"), tr_key("refraction"), tr_key("foam")],
    },
    // ---- effect ----
    Entry {
        kind: "fx.shake",
        description: tr_key("Shakes the whole picture in small jolts, like handheld camera shake."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.light_leak",
        description: tr_key("A screen-locked light leak washed over the picture, like light getting into film."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.ghost",
        description: tr_key("Ghosts the picture with a trail of earlier frames behind whatever moves."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.kaleido",
        description: tr_key("Folds the picture into a mirrored kaleidoscope with up to sixteen segments."),
        keypoints: [tr_key("segments"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.crt",
        description: tr_key("Gives the picture an old CRT look with scanlines and a phosphor glow."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.glitch",
        description: tr_key("Breaks the picture into displaced lines and blocks, with noise and inverted bits."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.posterize",
        description: tr_key("Quantises each colour channel into a few flat levels, like a screen print."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.zoom",
        description: tr_key("Pushes the picture in or pulls it out from the centre; negative values invert it."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.fire",
        description: tr_key("Lays a flickering flame gradient over the picture, rising from the bottom edge."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.lorenz",
        description: tr_key("Bends the picture along a Lorenz attractor, so the warp follows a chaotic path."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.vignette",
        description: tr_key("Darkens the corners and edges of the picture, pulling the eye to the centre."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.blur",
        description: tr_key("Softens the picture with a separable blur that keeps its overall shapes."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.sharpen",
        description: tr_key("Sharpens edges by boosting the difference between each pixel and its neighbours."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.grain",
        description: tr_key("Adds film grain over the picture, the same noise at every frame of a shot."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.grade",
        description: tr_key("A Lightroom-style colour grade: tone, white balance, curves and an eight-band mixer."),
        keypoints: [tr_key("temperature"), tr_key("exposure"), tr_key("saturation")],
    },
    Entry {
        kind: "fx.texture",
        description: tr_key("Adds or removes fine local contrast at the pixel scale, sharpening small detail."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.clarity",
        description: tr_key("Adds or softens mid-tone local contrast over a broader radius than texture."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.dehaze",
        description: tr_key("Removes or adds atmospheric haze, pulling contrast back into distant areas."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.denoise",
        description: tr_key("Reduces luminance and colour noise while keeping detail and smoothness in check."),
        keypoints: [tr_key("luminance"), tr_key("colour"), tr_key("detail")],
    },
    Entry {
        kind: "fx.optics",
        description: tr_key("Corrects lens distortion, vignetting and chromatic aberration."),
        keypoints: [tr_key("distortion"), tr_key("vignetting"), tr_key("aberration")],
    },
    Entry {
        kind: "fx.geometry",
        description: tr_key("Reshapes the picture with perspective, rotation, scale and offset, like a crop."),
        keypoints: [tr_key("perspective"), tr_key("rotate"), tr_key("scale")],
    },
    Entry {
        kind: "fx.fm",
        description: tr_key("A carrier wave displaces the picture along an angle, its phase bent by a modulator."),
        keypoints: [tr_key("carrier frequency"), tr_key("modulator frequency"), tr_key("index")],
    },
    Entry {
        kind: "fx.wavewarp",
        description: tr_key("A travelling wave displaces the picture sideways along its direction of travel."),
        keypoints: [tr_key("wave type"), tr_key("wavelength"), tr_key("speed")],
    },
    Entry {
        kind: "fx.ascii",
        description: tr_key("Redraws the picture as a grid of characters, each glyph picked by its cell's brightness."),
        keypoints: [tr_key("cell size"), tr_key("colour mode"), tr_key("invert")],
    },
    Entry {
        kind: "fx.brush",
        description: tr_key("Repaints the frame from oriented strokes, coarse first and finer after."),
        keypoints: [tr_key("stroke size"), tr_key("coverage"), tr_key("paper")],
    },
    Entry {
        kind: "fx.hardstutter",
        description: tr_key("A beat-locked gate that hides or holds frames, for fast stutter edits."),
        keypoints: [tr_key("rhythm"), tr_key("duty"), tr_key("tempo")],
    },
    Entry {
        kind: "fx.forge",
        description: tr_key("Runs a WGSL shader, written by the operator or the AI, over the picture it is handed."),
        keypoints: [tr_key("shader file"), tr_key("control input"), tr_key("image input")],
    },
    Entry {
        kind: "fx.speed",
        description: tr_key("Changes a generator's clock from now on, slowing or speeding its motion without jumps."),
        keypoints: [tr_key("speed"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.move",
        description: tr_key("Shifts the picture against its mask, drifting per beat while the mask stays put."),
        keypoints: [tr_key("offset"), tr_key("drift"), tr_key("wrap")],
    },
    Entry {
        kind: "fx.particles",
        description: tr_key("Stateless dust of drifting points with twinkle, jitter and depth parallax."),
        keypoints: [tr_key("count"), tr_key("drift"), tr_key("twinkle")],
    },
    Entry {
        kind: "fx.image-pop",
        description: tr_key("Pops reel stills and clips over the picture at random spots, fading in and out."),
        keypoints: [tr_key("concurrent"), tr_key("life"), tr_key("scale")],
    },
    Entry {
        kind: "fx.warp",
        description: tr_key("Bends the picture through a smooth displacement field, shaped by mode and frequency."),
        keypoints: [tr_key("intensity"), tr_key("frequency"), tr_key("mosh")],
    },
    Entry {
        kind: "fx.flash",
        description: tr_key("A white flash that keeps the picture visible, the strobe an effect loop fires on the beat."),
        keypoints: [tr_key("opacity"), tr_key("diffuse"), tr_key("reactive control")],
    },
    Entry {
        kind: "fx.text.typewriter",
        description: tr_key("Reveals the glyphs of a text one by one, like typing across the line."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.text.wave",
        description: tr_key("Lifts the letters of a text in a travelling wave, each one offset by its index."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.text.jitter",
        description: tr_key("Shakes each letter of a text a little at random, like unstable type."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.text.color_cycle",
        description: tr_key("Cycles the hue of each letter of a text, shifted by its position in the line."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.text.scale_pulse",
        description: tr_key("Pulses the size of each letter of a text, shifted by its position in the line."),
        keypoints: [tr_key("intensity"), tr_key("reactive control"), tr_key("fixed value")],
    },
    Entry {
        kind: "fx.chroma_key",
        description: tr_key("Keys out a colour by tolerance and softness, letting whatever lies behind show."),
        keypoints: [tr_key("tolerance"), tr_key("softness"), tr_key("spill")],
    },
    Entry {
        kind: "fx.level",
        description: tr_key("Remaps the black and white points, gamma, brightness and contrast of the picture."),
        keypoints: [tr_key("gamma"), tr_key("input black"), tr_key("input white")],
    },
    Entry {
        kind: "fx.lookup",
        description: tr_key("Remaps the picture's colours through a lookup table, from a cube file or a palette."),
        keypoints: [tr_key("channel"), tr_key("mix"), tr_key("source")],
    },
    Entry {
        kind: "fx.math",
        description: tr_key("Multiplies, adds and raises each colour channel to a power, with optional absolute value."),
        keypoints: [tr_key("multiply"), tr_key("add"), tr_key("power")],
    },
    Entry {
        kind: "fx.projection",
        description: tr_key("Projects the picture through a virtual camera, rotated by yaw, pitch and roll."),
        keypoints: [tr_key("yaw"), tr_key("pitch"), tr_key("field of view")],
    },
    // ---- audio ----
    Entry {
        kind: "audio.live",
        description: tr_key("Live audio from the input device, turned into the per-frame digest that drives controls."),
        keypoints: [tr_key("input device"), tr_key("digest columns"), tr_key("live only")],
    },
    Entry {
        kind: "audio.file",
        description: tr_key("A song file decoded and analysed into the same digest the live input would give."),
        keypoints: [tr_key("song file"), tr_key("digest columns"), tr_key("offline only")],
    },
    Entry {
        kind: "clock.timecode",
        description: tr_key("Reads SMPTE timecode from LTC, MIDI or Art-Net and drives the sequence playhead."),
        keypoints: [tr_key("ltc input"), tr_key("midi timecode"), tr_key("art-timecode")],
    },
    // ---- reel and picture sources ----
    Entry {
        kind: "reel.clip",
        description: tr_key("One video or still clip from the reel library, played back as this node's picture."),
        keypoints: [tr_key("clip"), tr_key("control input"), tr_key("image output")],
    },
    Entry {
        kind: "reel.batch",
        description: tr_key("A multi-source reel of videos, stills, scenes and favourite groups, stepped on a clock."),
        keypoints: [tr_key("items"), tr_key("interval"), tr_key("order")],
    },
    Entry {
        kind: "still",
        description: tr_key("A single image from disk, held as the picture until the graph changes."),
        keypoints: [tr_key("image file"), tr_key("image output"), tr_key("library")],
    },
    Entry {
        kind: "text.source",
        description: tr_key("A string rendered to a frame, with typography and per-glyph effects applied to it."),
        keypoints: [tr_key("content"), tr_key("font family"), tr_key("control input")],
    },
    Entry {
        kind: "capture",
        description: tr_key("A live camera, screen or program window, streamed in as frames like any picture."),
        keypoints: [tr_key("camera or screen"), tr_key("live frames"), tr_key("image output")],
    },
    // ---- routing, scripts and masks ----
    Entry {
        kind: "router",
        description: tr_key("Puts one of its image inputs on air, switching only at the song's section changes."),
        keypoints: [tr_key("image inputs"), tr_key("section changes"), tr_key("single")],
    },
    Entry {
        kind: "script.py",
        description: tr_key("A Python script that rewrites a control row on every frame."),
        keypoints: [tr_key("source code"), tr_key("floats"), tr_key("control row")],
    },
    Entry {
        kind: "script.rs",
        description: tr_key("A Rust script that rewrites a control row on every frame."),
        keypoints: [tr_key("source code"), tr_key("floats"), tr_key("control row")],
    },
    Entry {
        kind: "script.js",
        description: tr_key("A JavaScript script that rewrites a control row on every frame."),
        keypoints: [tr_key("source code"), tr_key("floats"), tr_key("control row")],
    },
    Entry {
        kind: "script.fader",
        description: tr_key("A manual fader that outputs a value moving between its low and high points."),
        keypoints: [tr_key("value"), tr_key("low"), tr_key("high")],
    },
    Entry {
        kind: "mask",
        description: tr_key("Keeps the picture only inside a shape, with an optional stroke along its edge."),
        keypoints: [tr_key("shape"), tr_key("stroke"), tr_key("control input")],
    },
    Entry {
        kind: "venue.surface",
        description: tr_key("Lays the picture onto one surface of a venue, one level in from the mask."),
        keypoints: [tr_key("venue"), tr_key("surface"), tr_key("image input")],
    },
    Entry {
        kind: "venue.fixtures",
        description: tr_key("Samples the frame at each fixture's footprint in a venue and sends each fixture's wire."),
        keypoints: [tr_key("venue"), tr_key("group"), tr_key("image input")],
    },
    // ---- output and export ----
    Entry {
        kind: "output.device",
        description: tr_key("Sends the picture to a screen or projector on this machine's graphics card."),
        keypoints: [tr_key("screen device"), tr_key("image input"), tr_key("control input")],
    },
    Entry {
        kind: "out.ddp",
        description: tr_key("Sends the picture as RGB strips over UDP to bulbs and LED controllers."),
        keypoints: [tr_key("udp"), tr_key("rgb strips"), tr_key("image input")],
    },
    Entry {
        kind: "out.json",
        description: tr_key("Sends each lighting state as a JSON request to a bulb's local HTTP API."),
        keypoints: [tr_key("json body"), tr_key("local http"), tr_key("signal input")],
    },
    Entry {
        kind: "out.sacn",
        description: tr_key("Sends DMX512 over the network as E1.31 sACN, one universe of 512 channels."),
        keypoints: [tr_key("universe"), tr_key("512 channels"), tr_key("image input")],
    },
    Entry {
        kind: "out.osc",
        description: tr_key("Sends addressed Open Sound Control messages to consoles and media servers."),
        keypoints: [tr_key("address"), tr_key("udp"), tr_key("signal input")],
    },
    Entry {
        kind: "out.http",
        description: tr_key("Sends one HTTP request per change, for fixtures that only offer a web endpoint."),
        keypoints: [tr_key("url"), tr_key("signal input"), tr_key("per change")],
    },
    Entry {
        kind: "out.midi",
        description: tr_key("Sends notes and control changes to MIDI instruments or consoles."),
        keypoints: [tr_key("notes"), tr_key("control change"), tr_key("signal input")],
    },
    Entry {
        kind: "out.bluetooth",
        description: tr_key("Drives Godox Bluetooth Mesh lights with a colour on each change, through a fixture profile."),
        keypoints: [tr_key("fixture profile"), tr_key("colour"), tr_key("signal input")],
    },
    Entry {
        kind: "out.serial",
        description: tr_key("Drives an Adalight LED strip over USB serial, sampling the picture along its length."),
        keypoints: [tr_key("port"), tr_key("pixels"), tr_key("baud")],
    },
    Entry {
        kind: "out.artnet",
        description: tr_key("Sends Art-Net 4 DMX to a port address, unicast or broadcast, with optional sync."),
        keypoints: [tr_key("universe"), tr_key("unicast"), tr_key("sync")],
    },
    Entry {
        kind: "export.video",
        description: tr_key("Renders the graph offline into a video file, frame by frame."),
        keypoints: [tr_key("image input"), tr_key("offline only"), tr_key("video file")],
    },
    // ---- sensors, cameras and colour ----
    Entry {
        kind: "sensor.yolo",
        description: tr_key("Runs YOLO object detection on the picture and emits what it finds as control values."),
        keypoints: [tr_key("detections"), tr_key("image input"), tr_key("control output")],
    },
    Entry {
        kind: "cam.track",
        description: tr_key("Writes a tracked camera position into the camera floats of the generators it names."),
        keypoints: [tr_key("control input"), tr_key("generator targets"), tr_key("camera position")],
    },
    Entry {
        kind: "color",
        description: tr_key("A colour you cable to a colour slot of a generator, so the slot follows the graph."),
        keypoints: [tr_key("colour value"), tr_key("control output"), tr_key("colour slot")],
    },
    Entry {
        kind: "palette",
        description: tr_key("A named four-slot palette cabled to colour slots, one colour for each role."),
        keypoints: [tr_key("base"), tr_key("accent"), tr_key("flash")],
    },
    Entry {
        kind: "ai.colourist",
        description: tr_key("A trainable colour model that proposes a grade for the generator it feeds."),
        keypoints: [tr_key("trained model"), tr_key("control input"), tr_key("control output")],
    },
    // ---- value sources and control rows ----
    Entry {
        kind: "source.osc",
        description: tr_key("A wave generator, sine, square, triangle or saw, that outputs a value over time."),
        keypoints: [tr_key("frequency"), tr_key("amplitude"), tr_key("sync")],
    },
    Entry {
        kind: "source.const",
        description: tr_key("A fixed number, or a named constant such as pi, e or tau."),
        keypoints: [tr_key("value"), tr_key("constant"), tr_key("control output")],
    },
    Entry {
        kind: "source.random",
        description: tr_key("A seeded random stream held at a rate, smoothed and shaped between a min and max."),
        keypoints: [tr_key("rate"), tr_key("min and max"), tr_key("distribution")],
    },
    Entry {
        kind: "source.noise",
        description: tr_key("Smooth value noise sampled along time or space, summed over several octaves."),
        keypoints: [tr_key("octaves"), tr_key("rate"), tr_key("axis")],
    },
    Entry {
        kind: "ctl.math",
        description: tr_key("Remaps a control row through a range, a curve, quantising and smoothing, then inverts."),
        keypoints: [tr_key("input range"), tr_key("curve"), tr_key("steps")],
    },
    Entry {
        kind: "in.artnet",
        description: tr_key("Reads Art-Net DMX in as a row of values that bindings can read as out0, out1."),
        keypoints: [tr_key("value count"), tr_key("control output"), tr_key("network")],
    },
    // ---- materials (planned) ----
    Entry {
        kind: "mat.matte",
        description: tr_key("A flat, non-reflective colour laid over a 3D part, with no texture of its own."),
        keypoints: [tr_key("mix"), tr_key("rough"), tr_key("tint")],
    },
    Entry {
        kind: "mat.concrete",
        description: tr_key("A grey, pitted concrete surface built from noise, with bumps across the part."),
        keypoints: [tr_key("scale"), tr_key("bump"), tr_key("rough")],
    },
    Entry {
        kind: "mat.wood",
        description: tr_key("A wood grain drawn from analytic rings and fibres across the part's surface."),
        keypoints: [tr_key("scale"), tr_key("bump"), tr_key("tint")],
    },
    Entry {
        kind: "mat.leather",
        description: tr_key("A grained, creased leather with a soft sheen, tinted like hide."),
        keypoints: [tr_key("bump"), tr_key("rough"), tr_key("tint")],
    },
    Entry {
        kind: "mat.metal",
        description: tr_key("A reflective metal finish whose roughness and environment reflection set its look."),
        keypoints: [tr_key("metal"), tr_key("rough"), tr_key("env")],
    },
    Entry {
        kind: "mat.glass",
        description: tr_key("A clear, reflective glass finish that blends the surroundings over the part."),
        keypoints: [tr_key("env"), tr_key("mix"), tr_key("tint")],
    },
    Entry {
        kind: "mat.matrix",
        description: tr_key("The grid look as a material, an object-space line pattern any 3D model can wear."),
        keypoints: [tr_key("scale"), tr_key("glitch"), tr_key("mix")],
    },
];
