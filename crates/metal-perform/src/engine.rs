//! The frame engine — `resolve → compile → execute → output` (LAY.9b).
//!
//! Everything between "what the operator asked for" and "the bytes of one
//! frame": the app's own state ([`State`]), the mode/composition resolution
//! ([`compile_device_plan`]) and the plan-fed compositor ([`render_device`]).
//! The per-layer walk it superseded is gone — LAY.8b froze the conformance
//! corpus into golden frames (see `GOLDEN` in this file's tests), which is
//! the gate that outlived it.
//!
//! **Why it lives here and not in the binary.** `metal-dash` keeps the HTTP
//! routes and the window; this crate keeps the engine. The corpus that has to
//! prove every frame pixel-identical can then run without starting a server,
//! a window or an event loop — and both builds (with and without a GPU) share
//! one execution path. See `docs/plan-architecture.md` D6/LAY.9.
//!
//! Nothing in this module knows a route, a socket or a window exists. The
//! graph files are the one thing it touches outside memory: a device's
//! authored canvas (`data/graphs/device-<id>.json`) is read per frame, which
//! is the same read `metal-dash` used to make from its own copy of this code.

use std::collections::HashMap;
use web_time::Instant;

use serde_json::{json, Value};

use metal_schema::Post;

use metal_logic::{run_with_budget, AuxError, AuxInput, AuxLogic, AuxOutput, Budget, FloatDecl, LogicContext};

use crate::compose;
use crate::digest;
use crate::drive::{self, Drive};
use crate::escena;
use crate::layers::{self, Blend, Collections, Layer, Limits, Sector};
use crate::listeners::{Digests, Listeners};
use crate::live::Input;
use crate::node_layer;
use crate::output::{self, Mask, Outputs};
use crate::overlay;
use crate::paint::Paint;
use crate::reel::Reels;
use crate::router::Router;
use crate::sequence;
use crate::switch;

/// `Cli.node`'s default and `State.local_node`'s initial value: first
/// non-empty of `/etc/hostname`, `$HOSTNAME`, else `"local"` — no new crate
/// dependency just to name this machine.
pub fn local_node_default() -> String {
    if let Ok(text) = metal_vfs::read_to_string("/etc/hostname") {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    "local".to_string()
}

pub struct State {
    /// The row the operator has set by hand for each model, which driving
    /// moves away from and `depth 0` returns to.
    pub base: Vec<Vec<f32>>,
    /// What is on air. **Not a `usize` any more**: it is a mode, a group and a
    /// history, and the one thing it still produces is an index. Everything
    /// downstream of it is unchanged, because the rest of the app was already a
    /// pure function of `state.time()` — which is what made the Router a module
    /// rather than a rewrite.
    pub router: Router,
    /// The screens, and what shape the picture is on each of them.
    pub outputs: Outputs,
    /// This machine's own name, for [`Output::composer`]/`shown_on` (S.R) —
    /// the hostname by default, overridable with `--node`.
    pub local_node: String,
    /// This machine's own listening port, for `POST /monitor {op:"open"}` to
    /// hand to the child it spawns — set from `cli.port` in `serve_forever`,
    /// never read from `Cli` again after that.
    pub port: u16,
    /// The built-in preview, as a device with a layer stack of its own.
    ///
    /// *"When no collection exists yet, auto mix is set on a device that is a
    /// preview."* — the preview **is** that device. It is
    /// not in `outputs.json` because it is not a screen in a room; it is the
    /// one every laptop has, it cannot be removed, and it is where a stack gets
    /// built before it is put on anything an audience can see.
    pub preview: layers::Device,
    /// Devices — composition and mode only, keyed by output id. Extracted
    /// from Output containment as part of 17.1 (invert containment).
    pub sources: HashMap<String, layers::Device>,
    /// Every layer, regardless of which device(s) it feeds — the second half
    /// of 17.1's inversion, and the part that actually enables fan-out
    /// (the design notes PARTE IV §17.1/§17.4). A `Layer` used to live inside exactly
    /// one `Device`'s own `Vec`, which is what made "one source, two
    /// destinations" impossible: it could not be in two `Vec`s. It now lives
    /// here, once, and finds its device(s) through [`layers::Layer::outputs`]
    /// — see [`State::layers_on`] and [`State::routed_to`].
    pub layers: Vec<Layer>,
    /// Saved setups, applicable to any device.
    pub collections: Collections,
    /// Saved timelines of collections (the design notes PARTE IV §17.5, second step) —
    /// the authored resource. What is actually armed on a device lives in
    /// `runs`, not here; see `sequence`'s module note on why the two are
    /// separate the same way `collections` and `overlays` are.
    pub sequences: sequence::Sequences,
    /// Saved switches (`S.AK`/`O.28`): grid buttons that flip a batch of
    /// nodes' `bypass` in one save. On/off is never stored here — see
    /// `switch::Switch::state` — so this is the same kind of resource
    /// `sequences` is: authored content, read fresh, with all the actual
    /// state living in the graphs it names.
    pub switches: switch::Switches,
    /// Parity P-E: when a switch last flipped each node — the one instant the
    /// graph cannot hold (see `transition`'s module docs). In memory only.
    pub transitions: crate::transition::Toggles,
    /// The shipped/operator transition presets (`data/transitions/*.json`).
    pub transition_presets: Vec<crate::transition::Preset>,
    pub limits: Limits,
    /// Footage: the library, the Curator, the Colourists, and what is cued.
    ///
    /// One of them for the whole app, beside the one Router and the one digest.
    /// Every screen showing a transmission is handed the same cue — two screens
    /// each deciding would drift apart within a phrase and the room would be
    /// watching two shots of one folder.
    pub reels: Reels,
    /// Saved movements running over live layers (§16.7).
    ///
    /// In the air only, never on disk — unlike every other field here it is
    /// deliberately not persisted. A phrase triggered during a set is a
    /// gesture, and an app that came back up still playing something the
    /// operator fired an hour ago would be restoring a mistake.
    pub overlays: overlay::Overlays,
    /// Sequences currently armed, at most one per device (§17.5, second
    /// step). Same discipline as `overlays` and for the same reason — see
    /// `sequence::Runs`'s own doc comment.
    pub runs: sequence::Runs,
    /// V4: one face solver per `gen.*` node, keyed by node id. Its springs are
    /// the coarticulation state, so it must outlive a frame — but only for the
    /// set: a face that came up in the morning still on last night's vowel
    /// would be a mistake, so it is never persisted, like `overlays`/`clocks`.
    /// See [`crate::face::apply`].
    pub faces: HashMap<metal_graph::NodeId, crate::face::Face>,
    /// H3: the latest `Mode::Eyes` reading per `sensor.yolo` node, in the air
    /// only. The detector that fills it lives with the capture frame (a
    /// backend, `metal-sense`); the engine only reads it, so a sensor whose
    /// camera is off is simply absent and `cam.track` writes nothing.
    pub head: HashMap<metal_graph::NodeId, metal_sense::Control>,
    /// H3: one detector per `sensor.yolo` node, kept across frames so its
    /// face selection and One Euro smoothing survive. The runtime builds a
    /// real backend behind `metal-sense`'s `mediapipe` feature and falls back to
    /// a silent stand-in; a test inserts a scripted `StandInDetector` here.
    /// `Send` because `State` is (`metal-dash`'s shared handle).
    pub head_detectors: HashMap<metal_graph::NodeId, Box<dyn metal_sense::Detector + Send>>,
    /// H3: a frame a test handed in for a `capture` node, keyed by that node's
    /// id. Never set on the live path, where the frame comes from the device
    /// (`crate::capture::frame`); it is the seam that lets
    /// `a_captured_frame_feeds_the_detector_and_moves_the_camera` run without
    /// a camera or `ffmpeg`.
    pub head_frames: HashMap<metal_graph::NodeId, metal_sense::Image>,
    /// H5: the `min_cutoff`/`beta` each `sensor.yolo` detector was built with,
    /// so a change in the node's config rebuilds it — the One Euro filter has
    /// no setter on the [`metal_sense::Detector`] trait, so the engine keeps the
    /// last-used dials and drops the detector when they part.
    pub head_smoothing: HashMap<metal_graph::NodeId, (f32, f32)>,
    pub post: Post,
    pub started: Instant,
    /// Where the clock was stopped, if it is. Everything downstream is a
    /// function of this one number, so pausing freezes the digest, the
    /// mapping and the render together rather than three things separately.
    pub paused_at: Option<f32>,
    pub selected_input: Option<String>,
    pub selected_display: Option<String>,
    /// S.AL.1: one open [`crate::wire::Wire`] per `out.sacn` node, kept
    /// alive across frames — [`crate::wire::drive_frame`] creates, reuses
    /// and drops these; `render_device` never touches the map itself beyond
    /// handing it over. Keyed by [`metal_graph::NodeId`] alone, the same key
    /// `render_device`'s own `rows`/`script_rows` maps use for one device's
    /// graph — see that call's own note on why a node id collision across
    /// two different devices' graphs is a known, harmless-in-practice edge
    /// (the mismatched entry is just rebuilt, never mis-addressed).
    pub wire_sockets: HashMap<metal_graph::NodeId, crate::wire::WireSocket>,
    /// BLE Phase B: one open [`metal_ble::Link`] per `out.bluetooth` node, kept
    /// alive across frames — [`crate::wire::drive_frame`] creates, reuses and
    /// drops these exactly like `wire_sockets`. Boxed behind the trait so a
    /// test can swap in a `metal_ble::Recorder` without a radio.
    pub bluetooth_links: HashMap<metal_graph::NodeId, crate::wire::BluetoothLink>,
    /// How many `out.sacn` sends have failed since boot — a non-blocking
    /// socket that would have blocked, or any other I/O error. Never a hard
    /// failure (regla: never block the frame), just a counter an operator
    /// or the properties panel can notice climbing.
    pub wire_errors: u64,
    /// The open wires of the frame-driven `out.*` nodes (a venue's washes,
    /// spots and LED strips), keyed `"<graph id>/<node id>"` —
    /// [`crate::wire::drive_device`] creates, reuses and drops these.
    pub sampled_wires: HashMap<String, crate::wire::SampledWire>,
    /// D3: the shared DMX universe bus every `out.*`/`venue.fixtures` source
    /// writes into, merged per source with priority. Like `sampled_wires`, it
    /// is live state a frame produces, never saved; the priority/mode it
    /// merges with are a reading of the source nodes' own config (rule 4).
    pub dmx_bus: crate::dmx_bus::DmxBus,
    /// Param edges' live values (`crate::bind`): per rendered graph, every
    /// node's parameters and variables as the last frame computed them —
    /// what `GET /graph/live` and `GET /graph/io` read. Written by
    /// [`render_device`] every frame; never saved.
    pub live: crate::bind::Live,
    /// K: each `(device, generator)`'s accumulated, speed-scaled clock
    /// (`fx.speed`) — see [`crate::motion::Clocks`]. In the air only, like
    /// `overlays`: a clock is a running thing, and restoring one from disk
    /// would resume a show mid-phase.
    pub clocks: crate::motion::Clocks,
    /// The one loop player for the process (plan step 7, "until then … one per
    /// process"): the router and every renderer take their frame from this, so
    /// the animation, the preview and the screens cannot disagree about what
    /// the loop is doing.
    ///
    /// **Not saved**, for the same reason `overlays`/`runs` are not: a mode and
    /// an engaged loop are the operator's hand on the machine this set, and a
    /// laptop that came back looping the previous night's phrase would be
    /// restoring a mistake (plan decision 6). **One per process for now** — once
    /// "Audio input is a node" lands it becomes per `audio.live` in
    /// `Listeners`, because a loop belongs to the source it is a loop *of*.
    pub looper: crate::looper::Looper,
    /// The one musical clock (the loops design notes T7): fed in [`State::on_air`] from
    /// the input's beat count, downbeat and tempo, read by every looped
    /// generator so they all agree on the "1".
    pub music: crate::clock::MusicClock,
    /// Where `music` said the bar was at the last `on_air` — what
    /// [`row_for_node`] evaluates loops at, so every row of one frame sees
    /// the same step.
    pub music_pos: crate::clock::Position,
    /// Factory + user loop templates — see [`loops_library`].
    pub loops: crate::pattern::Library,
    /// The song the sequence editor is playing, when it has a beat map
    /// (the loops design notes T15): while it is set, [`State::on_air`] feeds `music`
    /// from the song's sidecar instead of the input.
    pub song: Option<SongClock>,
}

/// A playing song's beat map and where its playback started — see
/// [`State::play_song`].
#[derive(Debug, Clone)]
pub struct SongClock {
    pub track: crate::clock::TempoTrack,
    /// Song seconds playback started from.
    pub from: f64,
    /// The engine time of the first `on_air` after the play — the moment the
    /// song's `from` is heard. Anchored lazily so the caller needs no clock.
    pub anchor: Option<f32>,
}

impl SongClock {
    /// Song time at engine `time` (anchoring on the first call).
    pub fn song_time(&mut self, time: f32) -> f64 {
        let anchor = *self.anchor.get_or_insert(time);
        self.from + (time - anchor).max(0.0) as f64
    }
}

impl State {
    pub fn new(paint: &dyn Paint) -> State {
        let outputs = Outputs::load();

        // 17.1/17.4: load devices (composition/mode) and the one flat layer
        // pool separately — load_devices() handles backward compatibility
        // with outputs.json shapes from every version of this app, including
        // ones written before a layer could be routed to more than one
        // device. See its own doc comment in output.rs.
        // (named `pool`, not `layers`, only so it does not shadow the
        // `layers` module for the rest of this function)
        let (mut sources, pool) = output::load_devices();

        // Conjunto E migration note (the design notes PARTE IV §17.4): a layer loaded
        // from an `outputs.json` written before `Layer::effects`/`fijo_mask`
        // existed has neither on disk, so serde defaults it to
        // `Post::default()` and `fijo_mask: 0` — fully reactive/off, not
        // "inherits whatever the master dial is right now". That is a real
        // choice, not just the path of least resistance: the alternative
        // (copying `post` below into every freshly-loaded layer with
        // `fijo_mask == 0`) would do exactly the same thing today, because
        // `post` on the next line is *also* always `Post::default()` — it is
        // never itself loaded from disk, only ever set at runtime through
        // `POST /drive`. Copying a default into a default is not a
        // migration, it is a second way to spell the first one, so the
        // simpler option (do nothing here) is also the more honest one: an
        // old layer says plainly "no effect was ever touched here" instead
        // of implying it once mirrored a dial that, this early in boot, has
        // not been touched either. If `post` is ever made to persist across
        // restarts, this note is exactly where to revisit that trade.

        // Ensure all outputs have a device in sources, using default if not found
        for out in &outputs.list {
            sources
                .entry(out.id.clone())
                .or_insert_with(layers::Device::default);
        }

        State {
            base: paint.schemas().iter().map(|s| s.defaults()).collect(),
            // Ids, not a count: the trained router names animations by id and
            // this is where one becomes the index everything else takes.
            // `set_data_root` has already run — see `main`.
            router: Router::new(
                paint.ids().iter().map(|s| s.to_string()).collect(),
                &output::data_root(),
            ),
            outputs,
            local_node: local_node_default(),
            port: 0,
            preview: layers::Device::default(),
            sources,
            layers: pool,
            collections: Collections::load(),
            sequences: sequence::Sequences::load(),
            switches: switch::Switches::load(),
            transitions: crate::transition::Toggles::default(),
            transition_presets: crate::transition::load_presets(),
            limits: Limits::default(),
            reels: Reels::new(&output::data_root()),
            overlays: overlay::Overlays::default(),
            runs: sequence::Runs::default(),
            faces: HashMap::new(),
            head: HashMap::new(),
            head_detectors: HashMap::new(),
            head_frames: HashMap::new(),
            head_smoothing: HashMap::new(),
            post: Post::default(),
            started: Instant::now(),
            paused_at: None,
            selected_input: None,
            selected_display: None,
            wire_sockets: HashMap::new(),
            bluetooth_links: HashMap::new(),
            wire_errors: 0,
            sampled_wires: HashMap::new(),
            dmx_bus: crate::dmx_bus::DmxBus::new(),
            live: crate::bind::Live::default(),
            clocks: crate::motion::Clocks::default(),
            looper: crate::looper::Looper::new(),
            music: crate::clock::MusicClock::new(),
            music_pos: crate::clock::Position::from_beats(0.0),
            loops: loops_library(),
            song: None,
        }
    }

    /// A song starts playing (or seeks) at song second `from` with its beat
    /// map: the musical clock follows the map from the next `on_air`. A seek
    /// is the one place the clock may go backwards ([`MusicClock::seek`]).
    pub fn play_song(&mut self, track: crate::clock::TempoTrack, from: f64) {
        self.music.seek();
        self.song = Some(SongClock { track, from: from.max(0.0), anchor: None });
    }

    /// The song stopped: the input feeds the clock again (never backwards
    /// from where the song left it).
    pub fn stop_song(&mut self) {
        self.song = None;
    }

    pub fn time(&self) -> f32 {
        self.paused_at
            .unwrap_or_else(|| self.started.elapsed().as_secs_f32())
    }

    /// Which animation is on air at `time`.
    ///
    /// Every path that draws or reports asks this rather than reading a field,
    /// so the preview, the monitor and every output are looking at the same
    /// animation. The router is idempotent at a given instant, which is what
    /// makes asking it four times a frame safe.
    pub fn on_air(&mut self, sim: &mut Input, time: f32) -> usize {
        // The trained router reads the music and not just a beat count, so it
        // is fed here rather than by a clock of its own. `Content::observe`
        // fills its buffer at the digest's 8 Hz however often this is called,
        // which matters because this is called several times a frame.
        //
        // The frame it sees is the **looped** one, not the live digest: the
        // router has to route to what the room is hearing, and once a loop is
        // engaged the room is hearing the replay. `poll` is indexed by beat
        // phase (looper.rs), so calling it here and again in the renderers at
        // the same `time` returns the same frame — the one Looper holds the
        // loop and every consumer reads the same instant of it.
        let State { router, looper, .. } = self;
        router.observe(time, |t| looper.poll(sim, t));
        // The detector lock (Phase 2) is fed once per `on_air`. `observe` with
        // the same value is harmless by design (it is a `Copy` assignment plus,
        // in `Auto`, an idempotent engage), so how many times a frame is drawn
        // cannot change when the loop engages.
        let repeat = sim.repeat();
        looper.observe(repeat);
        let beats = sim.beats(time);
        // The musical clock is fed here for the same reason the router is:
        // every drawing path asks `on_air` first. `update` at the same `time`
        // is idempotent (never backwards, same count, same answer).
        //
        // A song playing from the sequence editor has its own beat map (its
        // `<song>.tempo.json`): the clock follows that, not the input, so
        // loops land on the song's "1"s from the first bar.
        self.music_pos = match self.song.as_mut() {
            Some(song) => {
                let t = song.song_time(time);
                let (count, downbeat, bpm) = song.track.at(t);
                self.music.update(count, downbeat, bpm, time as f64)
            }
            None => self.music.update(
                beats,
                sim.downbeat(),
                Some(sim.bpm()).filter(|b| *b > 0.0),
                time as f64,
            ),
        };
        self.router.on_air(time, beats)
    }

    /// The layer stack of any device, preview or screen, by id.
    pub fn stack(&self, id: &str) -> Option<&layers::Device> {
        if id == PREVIEW {
            Some(&self.preview)
        } else {
            self.sources.get(id)
        }
    }

    /// Every device id, preview first — the order the page lists them in.
    pub fn device_ids(&self) -> Vec<String> {
        let mut ids = vec![PREVIEW.to_string()];
        ids.extend(self.outputs.list.iter().map(|o| o.id.clone()));
        ids
    }

    pub fn stack_mut(&mut self, id: &str) -> Option<&mut layers::Device> {
        if id == PREVIEW {
            Some(&mut self.preview)
        } else {
            self.sources.get_mut(id)
        }
    }

    // -----------------------------------------------------------------------
    // filtering the layer pool by device — the design notes PARTE IV §17.1/§17.4
    // -----------------------------------------------------------------------
    //
    // Thin wrappers over the free functions in `layers.rs` (which take a pool
    // and a device id rather than `self`, so they stay usable — and testable
    // — without a whole `State`). These exist so call sites here read the way
    // `stack.draw_order()` and friends used to, now that `Device` itself has
    // nothing to walk.

    /// Every layer routed to `id`, unfiltered by active/solo — what the
    /// activity table lists and what a bulk replace (apply/clear/mode) acts
    /// on.
    pub fn routed_to(&self, id: &str) -> Vec<&Layer> {
        layers::routed_to(&self.layers, id)
    }

    /// The layers on `id` that actually draw, back to front by `z`. What
    /// `Device::draw_order` was before a layer could be routed to more than
    /// one device.
    pub fn layers_on(&self, id: &str) -> Vec<&Layer> {
        layers::layers_on(&self.layers, id)
    }

    /// The next free `z` among the layers on `id`.
    pub fn next_z_on(&self, id: &str) -> i32 {
        layers::next_z_on(&self.layers, id)
    }

    /// The layer with this id, in the global pool — ids are unique across
    /// every device now, not just within one, so this needs no device to
    /// search inside.
    pub fn find_layer_mut(&mut self, id: &str) -> Option<&mut Layer> {
        layers::find_mut(&mut self.layers, id)
    }

    /// What the automatic half is doing this instant, for every source to read.
    ///
    /// Built once and passed down rather than asked for per layer: `on_air` and
    /// `ranking` both mutate the router (they advance its clocks and its hold),
    /// and calling them once per source would mean a device with six sources
    /// runs the router six times a frame. It is idempotent at a given instant,
    /// so that would be harmless and still wrong — six sources on `automix`
    /// would each be reading a router that four of them had already advanced.
    pub fn airing(&mut self, sim: &mut Input, time: f32) -> layers::Airing {
        let on_air = self.on_air(sim, time);
        layers::Airing {
            on_air,
            step: self.router.step(),
            ranked: self.router.ranking(time),
        }
    }

    /// Every device the page knows about: the built-in preview first, then the
    /// configured screens, all the same shape.
    ///
    /// The preview is listed as a device rather than treated as chrome because
    /// the ask puts automix *on a preview device* — which only means anything
    /// if the preview is in the list you choose a mode for.
    pub fn devices(&self, air: &layers::Airing, models: usize) -> Value {
        let mut out = vec![{
            let mut v = json!({
                "id": PREVIEW,
                "label": "preview",
                "width": 0,
                "height": 0,
                "calibrating": false,
                // The one flag that separates it: no `/out/<id>` to open, no
                // contours to draw, and it cannot be removed.
                "preview": true,
                "masks": [],
            });
            if let (Some(dst), Value::Object(src)) = (
                v.as_object_mut(),
                self.preview
                    .describe(PREVIEW, &self.layers, &self.limits, air, models),
            ) {
                dst.extend(src);
            }
            v
        }];
        if let Some(list) = self
            .outputs
            .describe(&self.limits, air, models, &self.sources, &self.layers)
            .as_array()
        {
            out.extend(list.iter().cloned());
        }
        json!(out)
    }

    /// Drops a deleted contour's id off every source that was bound to it.
    ///
    /// Across every device, not only the one the contour was on: nothing stops
    /// a source being moved to another screen while it still names a contour
    /// that only existed on the first, and a binding left behind is a binding
    /// that comes back to life when an id is reused.
    ///
    /// **One pass over the flat pool, since the design notes PARTE IV §17.1/§17.4** —
    /// there is no longer a per-device `Vec` to loop over devices to reach.
    pub fn unbind(&mut self, mask: &str) {
        for l in self.layers.iter_mut() {
            if l.mask.as_deref() == Some(mask) {
                l.mask = None;
            }
        }
    }

    /// Drops a deleted collection's id off every device that was running it,
    /// keeping the layers and admitting they are the device's own now.
    pub fn orphan(&mut self, collection: &str) {
        let hit = |m: &layers::DeviceMode| matches!(m, layers::DeviceMode::Collection(c) if c == collection);
        if hit(&self.preview.mode) {
            self.preview.mode = layers::DeviceMode::Own;
        }
        // 17.1: Devices moved to sources HashMap
        for device in self.sources.values_mut() {
            if hit(&device.mode) {
                device.mode = layers::DeviceMode::Own;
            }
        }
    }
}

/// The id the built-in preview answers to everywhere a device is named.
///
/// Reserved: `/outputs add` refuses it, so a screen can never shadow the one
/// device that is guaranteed to exist.
pub const PREVIEW: &str = "preview";

/// B6: converts `data/collections.json` into `data/graphs/<id>.json`, once.
///
/// **`data/graphs/` existing at all is the "already done" marker** — no flag
/// file, no version stamp in `collections.json` itself, because the whole
/// point (`the design notes` §7, §10 Conjunto B6) is that
/// `collections.json` is never touched by this app again after this runs
/// once. An operator who deletes `data/graphs/` by hand gets a fresh
/// conversion on the next start, which is the honest behaviour for "I want
/// this redone", not a bug to guard against.
///
/// A missing `collections.json`, an unreadable `schema.json`, or a
/// conversion error are all silent-but-logged no-ops: this must never be
/// the reason the app fails to start, and the original file — the one real
/// source of truth until this succeeds — is untouched in every one of those
/// cases.
pub fn migrate_collections_to_graphs_once() {
    migrate_collections_to_graphs_at(&output::data_root());
}

/// The testable half of [`migrate_collections_to_graphs_once`] — split out
/// so a test can point it at a scratch directory instead of the process-wide
/// `output::data_root()`, which is a `OnceLock` and can only be set once per
/// process.
pub fn migrate_collections_to_graphs_at(root: &std::path::Path) {
    let graphs_dir = graphs_dir(root);
    if metal_vfs::exists(&graphs_dir) {
        return;
    }
    let Ok(collections_text) = metal_vfs::read_to_string(root.join("collections.json")) else {
        return;
    };
    let manifest = match metal_vfs::read_to_string(root.join("schema.json"))
        .ok()
        .and_then(|s| metal_schema::Manifest::parse(&s).ok())
    {
        Some(m) => m,
        None => {
            eprintln!(
                "  migrate: cannot read schema.json in {} — collections.json left as the only copy",
                root.display()
            );
            return;
        }
    };
    let graphs = match metal_graph::migrate::from_layers(&collections_text, &manifest) {
        Ok(g) => g,
        Err(e) => {
            eprintln!(
                "  migrate: collections.json did not convert ({e}) — left in place, untouched"
            );
            return;
        }
    };
    if let Err(e) = metal_vfs::create_dir_all(&graphs_dir) {
        eprintln!("  migrate: cannot create {}: {e}", graphs_dir.display());
        return;
    }
    let mut written = 0usize;
    for g in &graphs {
        let path = graphs_dir.join(format!("{}.json", g.id));
        match crate::atomic::write(&path, g.to_json()) {
            Ok(()) => written += 1,
            Err(e) => eprintln!("  migrate: cannot write {}: {e}", path.display()),
        }
    }
    eprintln!(
        "  migrate: converted {written} collection(s) from collections.json into {} — collections.json left in place",
        graphs_dir.display()
    );
}

/// LAY.7b: every device's `stack.mode` is written into its authored graph as
/// its one `compose.*` node, once, at boot — "the legacy reads through the
/// graph, a single source of truth".
///
/// **One-way, in place, and idempotent.** A graph that already carries a
/// `compose.*` node is left exactly as it is, including a mode the operator
/// has since set through the canvas — which is the whole reason the node, not
/// the file, wins in `compile_device_plan`. A legacy `stack.mode` node is
/// *converted*: the same node id becomes the compose kind, so a graph with
/// one mode node never grows a second and the canvas does not re-lay-out. A
/// device with no graph at all gets the same one-node canvas
/// `ensure_device_graph_at` writes on `POST /outputs {op:"add"}`, now
/// carrying its mode. `layers::Device::mode` stays in the file as the
/// pre-migration fallback (`graph_device_mode`'s own "never wrong, only
/// older"), so nothing here is destructive and a build that predates the
/// node keeps reading the file it always did.
///
/// `Disabled` retires the node — §2.4's mapping — and a device already
/// without one is not rewritten.
pub fn migrate_device_modes_to_graphs_at(
    root: &std::path::Path,
    modes: &[(String, layers::DeviceMode)],
) {
    for (id, mode) in modes {
        let gid = device_graph_id(id);
        let (mut graph, mut changed) = match load_graph_at(root, &gid) {
            Ok(g) => (g, false),
            Err(_) => {
                let mut g = metal_graph::Graph::new(&gid, id, metal_graph::Domain::Live);
                g.add_node("out", "output.device", [800.0, 0.0]);
                (g, true)
            }
        };
        // R22.5b: the mode now lives on the `output.device` node. A graph
        // that already declares one is the operator's answer (or this
        // migration's own past run) — never overwrite it.
        if migrate_legacy_automix(&mut graph) {
            if let Err(e) = save_graph_at(root, &graph) {
                eprintln!("  migrate: cannot save {gid}: {e}");
            }
        }
        if output_device_mode(&graph).is_some() {
            continue;
        }
        let legacy: Vec<String> = graph
            .nodes
            .iter()
            .filter(|n| metal_graph::is_compose_kind(&n.kind) || n.kind == metal_graph::STACK_MODE_KIND)
            .map(|n| n.id.clone())
            .collect();
        let had_legacy = !legacy.is_empty();
        // Nothing to convert and nothing to declare: a disabled device that
        // never had a node is already what §2.4 says it should be — it still
        // gets its canvas (every device has one to open in the editor), just
        // no mode.
        if had_legacy || !matches!(mode, layers::DeviceMode::Disabled) {
            // Rewire fan-in first: a `compose.*` mixer's members move directly
            // onto `output.device.image` before the node is retired, so a
            // graph that mixed through it keeps drawing the same members.
            let out_id = graph
                .nodes
                .iter()
                .find(|n| n.kind == "output.device")
                .map(|n| n.id.clone());
            if let Some(out_id) = &out_id {
                let mut new_edges: Vec<(String, String)> = Vec::new();
                for comp in &legacy {
                    for e in graph
                        .edges
                        .iter()
                        .filter(|e| e.to.node == *comp && e.to.port == "image")
                    {
                        new_edges.push((e.from.node.clone(), e.from.port.clone()));
                    }
                    // If the compose fed the output, its incoming sources now
                    // land on the output directly.
                    let feeds_out = graph
                        .edges
                        .iter()
                        .any(|e| e.from.node == *comp && e.to.node == *out_id);
                    if feeds_out {
                        for (from_node, from_port) in &new_edges {
                            graph.add_edge(
                                metal_graph::Endpoint::new(from_node, from_port),
                                metal_graph::Endpoint::new(out_id, "image"),
                            );
                        }
                    }
                    new_edges.clear();
                }
            }
            if let Err(e) = set_graph_device_mode(&mut graph, mode) {
                eprintln!("  migrate: cannot write the mode for {id}: {e}");
                continue;
            }
            changed = true;
        }
        if changed {
            if let Err(e) = save_graph_at(root, &graph) {
                eprintln!("  migrate: cannot save {gid}: {e}");
            }
        }
    }
}

/// The boot-time half of [`migrate_device_modes_to_graphs_at`]: every device
/// the app knows about — the preview first, then each screen — with the mode
/// its `Device` currently holds.
pub fn migrate_device_modes_to_graphs_once(state: &State) {
    let modes: Vec<(String, layers::DeviceMode)> = state
        .device_ids()
        .into_iter()
        .filter_map(|id| state.stack(&id).map(|d| (id, d.mode.clone())))
        .collect();
    migrate_device_modes_to_graphs_at(&output::data_root(), &modes);
}

/// D1: the node/edge mutation surface `list_graphs`'s own doc comment named
/// as still missing. One file per graph (`data/graphs/<id>.json`), read and
/// written per request — no in-memory cache, the same choice `list_graphs`
/// already makes and for the same reason: this is a handful of requests a
/// human editor makes while looking at a canvas, not a pump.
pub fn graph_path(id: &str) -> std::path::PathBuf {
    graph_path_at(&output::data_root(), id)
}

pub fn graph_path_at(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    graphs_dir(root).join(format!("{id}.json"))
}

/// Test builds only: the graphs the scratch copy is seeded from, instead of
/// the real `data/graphs`. Set by [`set_test_graphs_source`].
static TEST_GRAPHS_SOURCE: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// The per-process scratch copy of the graphs dir (see [`graphs_dir`]).
static SCRATCH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Re-seed the test scratch graphs dir from `dir` (a committed fixture set)
/// instead of the real `data/graphs`. Safe to call late: a scratch that already
/// exists is wiped and re-seeded, so a test crate calls this from each test
/// that needs its fixtures and the order of tests does not matter.
pub fn set_test_graphs_source(dir: &std::path::Path) {
    *TEST_GRAPHS_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_path_buf());
    if let Some(scratch) = SCRATCH.get() {
        seed_scratch(scratch, dir);
    }
}

/// Wipe `scratch` and copy every non-device graph from `source` into it.
fn seed_scratch(scratch: &std::path::Path, source: &std::path::Path) {
    let _ = metal_vfs::remove_dir_all(scratch);
    metal_vfs::create_dir_all(scratch).expect("scratch graphs dir");
    if let Ok(entries) = metal_vfs::read_dir(source) {
        for e in entries.flatten() {
            // Device graphs are the operator's live state, not fixtures.
            if e.file_name().to_string_lossy().starts_with("device-") {
                continue;
            }
            let _ = metal_vfs::copy(e.path(), scratch.join(e.file_name()));
        }
    }
}

/// `<root>/graphs`, except in test builds, where the real data root's graphs
/// are a per-process scratch copy: tests read the repo's fixtures but must
/// never write the operator's canvases (a test once left the live preview
/// stuck on a collection).
pub fn graphs_dir(root: &std::path::Path) -> std::path::PathBuf {
    let real = root.join("graphs");
    if !cfg!(any(test, feature = "test-isolated-graphs")) || root != output::data_root() {
        return real;
    }
    SCRATCH
        .get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("fw-test-graphs-{}", std::process::id()));
            let source = TEST_GRAPHS_SOURCE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_else(|| real.clone());
            seed_scratch(&dir, &source);
            dir
        })
        .clone()
}

pub fn load_graph(id: &str) -> Result<metal_graph::Graph, String> {
    load_graph_at(&output::data_root(), id)
}

pub fn load_graph_at(root: &std::path::Path, id: &str) -> Result<metal_graph::Graph, String> {
    let path = graph_path_at(root, id);
    let text = metal_vfs::read_to_string(&path).map_err(|e| format!("no such graph {id:?}: {e}"))?;
    metal_graph::Graph::from_json(&text).map_err(|e| format!("{id:?} did not parse: {e}"))
}

/// O.8: `Err` (the refusal, as text) when another peer owns graph `id`.
pub fn check_graph_owner_at(root: &std::path::Path, id: &str) -> Result<(), String> {
    metal_graph::owner::check_write(&graphs_dir(root), id, &metal_graph::owner::local_identity())
        .map_err(|r| r.to_string())
}

pub fn save_graph(g: &metal_graph::Graph) -> Result<(), String> {
    save_graph_at(&output::data_root(), g)
}

pub fn save_graph_at(root: &std::path::Path, g: &metal_graph::Graph) -> Result<(), String> {
    // O.8: a graph another peer owns is refused here, the one chokepoint
    // every shell route and engine write goes through. Free = unchanged.
    check_graph_owner_at(root, &g.id)?;
    // One graph per collection: a collection graph publishes to the screens
    // its outputs name, and a write to a published `device-<id>` graph is an
    // edit of its collection (`metal_graph::collection`).
    let dir = graphs_dir(root);
    metal_graph::collection::store(
        &dir,
        g,
        &metal_graph::collection::Io {
            write: &|path, text| {
                if let Some(parent) = path.parent() {
                    metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                crate::atomic::write(path, text).map_err(|e| e.to_string())
            },
            check: &|id| check_graph_owner_at(root, id),
        },
    )
}

/// Boot: every device graph that has no collection yet joins the one central
/// collection (`metal_graph::collection::CENTRAL_ID`), so each screen opens as
/// a node in the same canvas. Idempotent.
pub fn adopt_device_graphs_at(root: &std::path::Path) -> Vec<String> {
    metal_graph::collection::adopt_device_graphs(
        &graphs_dir(root),
        &metal_graph::collection::Io {
            write: &|path, text| collection_write(path, text),
            check: &|id| check_graph_owner_at(root, id),
        },
    )
}

/// The `Io::write` every collection write goes through: create the parent
/// directory first (a scratch graphs dir may not exist yet — the boot
/// migrations run before anything else writes there), then write atomically.
fn collection_write(path: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(path, text).map_err(|e| e.to_string())
}

/// Boot: the per-device collections an earlier install minted (`col-<screen>`)
/// are folded into the central collection, keeping each screen's nodes and
/// output node. A no-op on a machine that never had them. Idempotent.
pub fn merge_device_collections_at(root: &std::path::Path) -> Vec<String> {
    metal_graph::collection::merge_device_collections(
        &graphs_dir(root),
        &metal_graph::collection::Io {
            write: &|path, text| collection_write(path, text),
            check: &|id| check_graph_owner_at(root, id),
        },
    )
}

/// A [`metal_graph::layout::Primitive`] as JSON, for a client that paints a
/// `<canvas>` from a flat list rather than depending on `metal-graph` itself —
/// the split D3 exists for (`the design notes` §5: "the browser receives
/// an already-resolved list of primitives"). Not `#[derive(Serialize)]` on the
/// crate's own type: `metal-graph`'s layout module has no reason to know its

/// A key for whether two layers' generators would render the same pixels:
/// the same `showing` model, the same float row and the same footage
/// library. Two layers with the same key share one `gen.*` node in
/// [`compile_device_plan`] rather than each minting their own —
/// `the design notes` §10 Conjunto B5's fan-out case.
///
/// Deliberately narrower than "the same `Layer`": two layers on different
/// feeds (`Feed::Model` vs `Feed::Automix`) that happen to be `showing` the
/// same thing this instant are still the same picture this frame, and there
/// is no reason to render it twice just because the operator got there by a
/// different control.
#[derive(Clone, PartialEq)]
struct GenKey {
    showing: usize,
    base: Option<Vec<f32>>,
    library: Option<String>,
    /// The rest of what makes two layers draw the same pixels. `base` is only
    /// the row before `apply_node_post`; the two fields below are what that
    /// call does to it, and `fondo_alpha` is the backdrop the `solo` call
    /// draws. Leaving any of them out folds two layers that disagree about a
    /// fixed effect, a `fijo_mask` or the backdrop toggle onto one `gen.*` node,
    /// and whichever row was inserted second silently wins for both — the
    /// exact failure class LAY.0b's corpus caught.
    effects: metal_schema::Post,
    fijo_mask: u32,
    fondo_alpha: bool,
    /// A `forge` source's WGSL: two forge layers with two shaders are two
    /// pictures, and folding them would draw one shader twice.
    shader: Option<String>,
}

/// The graph id a device's own authored canvas is saved under.
///
/// A plain naming convention over the graph store `POST /graph` already
/// writes to, rather than a second directory: a device graph *is* a graph,
/// editable by the same canvas and the same routes, and giving it its own
/// storage would mean two savers to keep in agreement.
pub fn device_graph_id(device_id: &str) -> String {
    format!("device-{device_id}")
}

/// `POST /outputs { op: "add" }`'s other half: make a freshly-added screen an
/// `output.device` node of the central collection, and publish the device graph
/// the engine runs from it (`metal_graph::collection`). A re-added screen that
/// is already central's, or a screen another collection owns, is left as it is,
/// so an operator-authored graph is never overwritten. `label` is unused: the
/// one collection keeps its own name.
pub fn ensure_device_graph_at(root: &std::path::Path, id: &str, _label: &str) {
    let _ = metal_graph::collection::ensure_device(
        &graphs_dir(root),
        id,
        &metal_graph::collection::Io {
            write: &|path, text| collection_write(path, text),
            check: &|gid| check_graph_owner_at(root, gid),
        },
    );
}

/// Each layer's generator, read off an authored graph in draw order.
///
/// Walks the `image` edges arriving at `output.device` in edge order — which
/// [`metal_graph::Graph::to_json`] sorts by id, so it is a stable property of the
/// file and not of insertion — and traces each one back upstream through any
/// `fx.*` nodes to the generator that starts that branch. The result lines up
/// one-to-one with the resolved layer list: the *n*-th thing arriving at the
/// output draws the *n*-th layer.
///
/// **LAY.7c: the walk descends through a `compose.*` mixer.** A branch that
/// reaches a compose node does not stop there — the mixer's own `image`
/// inputs *are* the members, so the walk gathers them, in the mixer's edge
/// order, and traces each upstream the same way. A wired `gen → compose →
/// output.device` therefore resolves to the same list as `gen →
/// output.device`; the compose node decides *how* the members are mixed, not
/// which of them exist.
///
/// A generator appearing twice is not an error and is not deduplicated here.
/// It is exactly how an authored graph spells the fan-out
/// [`GenKey`] produces when synthesizing — one generator, two edges into the
/// output — and collapsing it would lose a layer.
pub fn authored_generators(graph: &metal_graph::Graph) -> Vec<metal_graph::NodeId> {
    authored_generators_with_masks(graph)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// What a `mask` node binds its branch to: a 2D contour on the device, or a
/// wall in the scene (Task #8's 3D mask). One enum so the upstream walk can
/// carry either and every consumer can tell which it has.
#[derive(Clone, Debug, PartialEq)]
enum MaskBinding {
    /// `config.mask` — a [`crate::output::Mask::id`] on this device.
    Device(String),
    /// `config.wall` (+ optional `config.contour`) — a
    /// [`crate::escena::Wall`]/[`crate::escena::WallMask`] in the scene.
    Wall(crate::layers::WallBinding),
}

/// R22.6: the same ordered walk [`authored_generators`] makes, with the
/// contour each branch is cut by — `None` for a branch that reaches the
/// device through no `mask` node.
///
/// A `mask` node (`metal_graph::NodeFamily::Mask`, label "Mask") sits between
/// the chain and the device: `gen → fx… → mask → output.device`. Everything
/// upstream of it is cut by the contour its config names (`config.mask`), the
/// same key `apply_script_masks` reads. Two branches through two `mask` nodes
/// are two cuts; a branch that reaches the device directly is uncut, exactly
/// as it was before this node could be wired in.
///
/// The mask is a level inherited on the way *down*: nested `mask` nodes would
/// be the nearer one (last seen wins), which is the only reading of "the mask
/// just before the device" that keeps a single id per branch.
pub fn authored_generators_with_masks(
    graph: &metal_graph::Graph,
) -> Vec<(metal_graph::NodeId, Option<String>)> {
    authored_generators_with_bindings(graph)
        .into_iter()
        .map(|(id, binding, _surface)| {
            let mask = match binding {
                // The 2D id is all this older reader ever carried; a wall
                // binding is not a device contour, so it reads as uncut here.
                Some(MaskBinding::Device(mask)) => Some(mask),
                _ => None,
            };
            (id, mask)
        })
        .collect()
}

/// The walk behind [`authored_generators_with_masks`], carrying the whole
/// binding rather than just its 2D half.
///
/// A `mask` node now names either a device contour (`{"device", "mask"}`, the
/// R22.6 cut [`authored_generators_with_masks`] returns) or a wall
/// (`{"wall", "contour"?}`, Task #8's 3D binding). The two are one level in
/// the same inheritance: whichever a branch passes through on the way down is
/// the one it carries to its generator.
fn authored_generators_with_bindings(
    graph: &metal_graph::Graph,
) -> Vec<(
    metal_graph::NodeId,
    Option<MaskBinding>,
    Option<crate::layers::SurfaceBinding>,
)> {
    let Some(out) = graph.nodes.iter().find(|n| n.kind == "output.device") else {
        return Vec::new();
    };
    let mut out_ids: Vec<(
        metal_graph::NodeId,
        Option<MaskBinding>,
        Option<crate::layers::SurfaceBinding>,
    )> = Vec::new();
    for edge in graph
        .edges
        .iter()
        .filter(|e| e.to.node == out.id && e.to.port == "image")
    {
        collect_generators(graph, &edge.from.node, 0, None, None, &mut out_ids);
    }
    out_ids
}

/// The binding a `mask` node's config names, if it names one.
///
/// A wall is read first and wins: a node carrying both keys is a 3D node with
/// a stale 2D id, and the wall is the more specific statement of intent.
/// `mask_node_id` stays the 2D-only reader the stroke path and
/// [`graph_has_mask_node`] use, so their meaning is untouched.
fn mask_node_binding(node: &metal_graph::Node) -> Option<MaskBinding> {
    if node.kind != "mask" {
        return None;
    }
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    if let Some(wall) = cfg.get("wall").and_then(Value::as_str) {
        return Some(MaskBinding::Wall(crate::layers::WallBinding {
            wall: wall.to_string(),
            contour: cfg.get("contour").and_then(Value::as_str).map(str::to_string),
        }));
    }
    cfg.get("mask")
        .and_then(Value::as_str)
        .map(|m| MaskBinding::Device(m.to_string()))
}

/// The 2D contour a `mask` node's config names, if it names one — R22.6's
/// reader, the same `mask` key [`apply_script_masks`] already uses.
fn mask_node_id(node: &metal_graph::Node) -> Option<String> {
    match mask_node_binding(node) {
        Some(MaskBinding::Device(mask)) => Some(mask),
        _ => None,
    }
}

/// P2: the surface a `venue.surface` node's config names, if it names one —
/// the runtime reader for [`metal_graph::VENUE_SURFACE_KIND`]'s
/// `{ "venue": …, "surface": … }`, the same seam [`mask_node_binding`] draws
/// for `mask`. The device it lands on is never read here: it comes from the
/// `output.device` the branch reaches, so the venue document stays a reading
/// of the graph (regla 4).
fn surface_node_binding(node: &metal_graph::Node) -> Option<crate::layers::SurfaceBinding> {
    if node.kind != metal_graph::VENUE_SURFACE_KIND {
        return None;
    }
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    let surface = cfg.get("surface").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    Some(crate::layers::SurfaceBinding {
        venue: cfg.get("venue").and_then(Value::as_str).unwrap_or_default().to_string(),
        surface: surface.to_string(),
        sector: None,
    })
}

/// The upstream walk behind [`authored_generators`]: through `fx.*` (the
/// effect chain is pass-through for buffer identity) and *into* a `compose.*`
/// node's members. Bounded by the node count rather than trusting the graph to
/// be acyclic: `lint` rejects cycles, but this runs on a file that may have
/// been written by an older build, and a render path must not be able to spin
/// forever on one.
fn collect_generators(
    graph: &metal_graph::Graph,
    at: &str,
    depth: usize,
    binding: Option<&MaskBinding>,
    surface: Option<&crate::layers::SurfaceBinding>,
    out: &mut Vec<(
        metal_graph::NodeId,
        Option<MaskBinding>,
        Option<crate::layers::SurfaceBinding>,
    )>,
) {
    if depth > graph.nodes.len() {
        return;
    }
    let Some(node) = graph.node(at) else { return };
    if node.kind.starts_with("fx.") {
        let Some(up) = graph
            .edges
            .iter()
            .find(|e| e.to.node == at && e.to.port == "image")
        else {
            return;
        };
        collect_generators(graph, &up.from.node, depth + 1, binding, surface, out);
        return;
    }
    // P2/P-V: a `venue.surface` node is pass-through for buffer identity like
    // `fx.*`, and it marks the branch with the surface it lands on. Unlike
    // the `mask` binding below it is *not* a level that overwrites: a branch
    // is `gen → mask → venue.surface → device`, so it carries both the cut
    // and the surface to the generator.
    //
    // Its `image` port **fans in** (`metal_graph::VENUE_SURFACE_KIND`), one
    // node per output grid: every upstream source is gathered in edge order,
    // and its position in that order is the sector it is dealt. The walk
    // therefore branches once per edge instead of following a single one, and
    // the sector index rides on the binding so `graph_composed_layers` can
    // write it onto the layer. A source reaching the same surface twice gets
    // two slots, the same way a generator drawn twice is two layers.
    if let Some(surface_binding) = surface_node_binding(node) {
        let mut slot = 0usize;
        for edge in graph
            .edges
            .iter()
            .filter(|e| e.to.node == at && e.to.port == "image")
        {
            let mut binding_with_slot = surface_binding.clone();
            binding_with_slot.sector = Some(slot);
            collect_generators(
                graph,
                &edge.from.node,
                depth + 1,
                binding,
                Some(&binding_with_slot),
                out,
            );
            slot += 1;
        }
        return;
    }
    // R22.6: a `mask` node is pass-through for buffer identity, exactly like
    // `fx.*` — but it *marks* the branch behind it. The mask it names is what
    // cuts everything upstream, so it fires on the way down and is inherited
    // by every generator this branch gathers.
    if let Some(mask_binding) = mask_node_binding(node) {
        let Some(up) = graph
            .edges
            .iter()
            .find(|e| e.to.node == at && e.to.port == "image")
        else {
            return;
        };
        collect_generators(graph, &up.from.node, depth + 1, Some(&mask_binding), surface, out);
        return;
    }
    if metal_graph::is_compose_kind(&node.kind) {
        for edge in graph
            .edges
            .iter()
            .filter(|e| e.to.node == at && e.to.port == "image")
        {
            collect_generators(graph, &edge.from.node, depth + 1, binding, surface, out);
        }
        return;
    }
    out.push((at.to_string(), binding.cloned(), surface.cloned()));
}

/// LAY.7b: the member list a `compose.layers` graph declares, built from its
/// own `gen.*` nodes — the graph as the *origin* of the composition, not a
/// coverage-checked overlay on top of the pool.
///
/// Returns `None` when the graph wires no members at all: the delegation
/// case, where the pool's routing decides. That is every migrated device
/// until somebody draws a member, which is what makes the migration a
/// no-snapshot, no-desync change.
///
/// **Content comes from the pool layer a node names, not from the node.**
/// `metal_graph::migrate` writes `source_id` into a generator's config; a node
/// that names one gets that layer — `base`, `effects`, `library`, `feed` and
/// all — with the node's own [`node_layer::NodeLayerConfig`] applied on top,
/// exactly the override semantics the coverage path uses. A node that names
/// none is a member added in the canvas: built from the node itself (`gen.*`
/// kind → model, label → name, config → the five fields), full-frame and
/// normal.
///
/// **The order is the edge order**, the same walk [`authored_generators`]
/// makes: the composition is the graph's, so the graph's order is the draw
/// order. `active`/`solo` stay the pool layer's own — a node's `bypass` means
/// "skip this step in the chain", not "hide the member", and reading it as
/// the latter would un-bypass a hidden layer the first time a migrated device
/// got a composition node. Skipping the step is honoured here, though: a
/// bypassed `gen.*` (O.28 — a switch turned this mood/strobe off) is left
/// out of the composition for this frame, without touching the layer's own
/// `active` in the pool.
pub fn graph_composed_layers(
    graph: &metal_graph::Graph,
    pool: &[Layer],
    manifest_ids: &[&str],
) -> Option<Vec<Layer>> {
    let gens = authored_generators_with_bindings(graph);
    if gens.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(gens.len());
    for (gen_id, cut, surface) in &gens {
        let node = graph.node(gen_id)?;
        if node.bypass {
            continue;
        }
        // Parity P-E: a source with no model row (`gen.ramp`, `text.source`,
        // `reel.*`, `still`, `capture`) is a tile too — its pixels come from
        // its own buffer in `Paint::run`, so the model index only names a
        // slot and the transition pass reaches it like any generator.
        let showing = match node
            .kind
            .strip_prefix("gen.")
            .and_then(|m| manifest_ids.iter().position(|id| *id == m))
        {
            Some(i) => i,
            None if is_tile_source(&node.kind) => 0,
            None => return None,
        };
        let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
        let source_id = cfg.get("source_id").and_then(Value::as_str);
        let mut layer = match source_id.and_then(|sid| pool.iter().find(|l| l.id == sid)) {
            Some(l) => l.clone(),
            None => {
                let name = if node.label.is_empty() {
                    gen_id.clone()
                } else {
                    node.label.clone()
                };
                Layer::new(gen_id, &name, showing)
            }
        };
        node_layer::NodeLayerConfig::from_node(node).apply_to(&mut layer);
        // R22.6: the `mask` node between this branch and the device cuts it,
        // and wins over anything the `gen.*` node itself pinned — it is the
        // nearer statement of intent, the one the operator drew last. P2's
        // `venue.surface` node, when there is one, rides along with it.
        apply_binding(&mut layer, cut.as_ref(), surface.as_ref());
        out.push(layer);
    }
    Some(out)
}

/// One `gen.*` (or tile source) node of `graph` as a layer, whether or not an
/// output composes it: the focused-node preview's way in for a generator that
/// is not wired to a device. `None` for a node that does not draw on its own.
fn standalone_gen_layer(
    graph: &metal_graph::Graph,
    node_id: &str,
    pool: &[Layer],
    manifest_ids: &[&str],
) -> Option<Layer> {
    let node = graph.node(node_id)?;
    let showing = match node
        .kind
        .strip_prefix("gen.")
        .and_then(|m| manifest_ids.iter().position(|id| *id == m))
    {
        Some(i) => i,
        None if is_tile_source(&node.kind) => 0,
        None => return None,
    };
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    let source_id = cfg.get("source_id").and_then(Value::as_str);
    let mut layer = match source_id.and_then(|sid| pool.iter().find(|l| l.id == sid)) {
        Some(l) => l.clone(),
        None => {
            let name = if node.label.is_empty() { node_id.to_string() } else { node.label.clone() };
            Layer::new(node_id, &name, showing)
        }
    };
    node_layer::NodeLayerConfig::from_node(node).apply_to(&mut layer);
    Some(layer)
}

/// Image sources that are not a manifest model but still compose as a tile.
pub fn is_tile_source(kind: &str) -> bool {
    kind == metal_graph::RAMP_KIND
        || kind == metal_graph::TEXT_SOURCE_KIND
        || kind.starts_with("reel.")
        || kind == "still"
        || kind == "capture"
}

/// Apply a `mask` node's binding and a `venue.surface` node's binding to one
/// layer, for this frame only. The 2D half keeps writing [`Layer::mask`]; the
/// 3D half writes the transient [`Layer::mask_wall`] and [`Layer::surface`].
/// One function so the call sites (the pooled and the graph-composed list)
/// cannot read the bindings differently.
fn apply_binding(
    layer: &mut Layer,
    mask: Option<&MaskBinding>,
    surface: Option<&crate::layers::SurfaceBinding>,
) {
    match mask {
        Some(MaskBinding::Device(mask)) => layer.mask = Some(mask.clone()),
        Some(MaskBinding::Wall(wall)) => layer.mask_wall = Some(wall.clone()),
        None => {}
    }
    if let Some(surface) = surface {
        layer.surface = Some(surface.clone());
        // P-V: the slot the walk dealt this branch, and how many there are.
        // Asked of the venue here rather than counted in the walk, so a band
        // with more sources than cells still wraps onto the cells it has.
        layer.sector_of = surface.sector;
    }
}

/// PARTE IV §17.6 / plan step 3: the authored graph for this device, *if* it
/// covers the stack that `stack.mode`, the sequence and the overlays just
/// resolved. `None` means "synthesize instead", and every `None` below is a
/// deliberate refusal rather than an error.
///
/// **Why coverage is all-or-nothing.** The alternative — using the authored
/// graph for the layers it names and synthesizing the rest — produces a frame
/// that is half one model of the world and half the other, at which point
/// nobody can say what the device is showing or why. A stack that has moved
/// out from under its graph falls back whole, keeps the show running, and
/// leaves the operator's file untouched to fix on purpose.
///
/// **What the authored graph is allowed to decide, and what it is not.** It
/// decides *which* generators draw, in what order, and what `fx.*` chain sits
/// behind each — the whole content of the canvas. Blend/z/sector/mask/opacity
/// stay properties of the `Layer`, resolved before this is called, exactly as
/// plan step 2 requires — **but a `gen.*` node may now pin any of the five**
/// via [`node_layer::NodeLayerConfig`] (B3 paso 4), overriding the layer's own
/// value for this frame only. Absent from a node's config (every node written
/// before this existed) means "no opinion", so a graph with no config on any
/// node reproduces the layer list byte for byte — the pixel-identity gate
/// this step's own regression tests hold it to.
///
/// `mode` (automix/collection) is deliberately not one of the five: it
/// decides *which* layers this list even is, before this function is ever
/// called, so a node inside the list it produced cannot reach back and pick
/// a different one. See `node_layer`'s module doc.
///
/// **O.4 gave `mode` the construct it needed, one level up.** It is a
/// `stack.mode` node — [`metal_graph::NodeFamily::Stack`], no ports, outside the
/// image chain — read by [`compile_device_plan`] *before* the list exists.
/// The paragraph above is still exactly right about why it could not be one
/// of the five here; what changed is that it no longer has to be nowhere.
/// See [`graph_device_mode`].
///
/// That split is what lets automix, overlays and a running `sequence::Run`
/// keep working unchanged with a graph attached — they all operate on the
/// layer list, and this reads it (then lets each node adjust its own entry)
/// rather than replacing it wholesale.
///
/// The `_at` half, so a test can point this at a scratch directory instead of
/// the repo's real `data/` — the same split `load_graph`/`load_graph_at`
/// already uses, and for the same reason: `DATA_ROOT` is a `OnceLock` set once
/// at startup, so a test that overrode it would decide the root for every
/// other test in the process.
pub fn authored_device_plan_at(
    root: &std::path::Path,
    device_id: &str,
    list: &[Layer],
    air: &layers::Airing,
    models: usize,
    manifest_ids: &[&str],
) -> Option<(
    Vec<Layer>,
    metal_graph::Graph,
    metal_graph::Plan,
    Vec<metal_graph::NodeId>,
)> {
    authored_device_plan_from(
        device_graph_at(root, device_id)?,
        list,
        air,
        models,
        manifest_ids,
    )
}

thread_local! {
    /// The graph a preview was asked for in (the canvas's open one), when it
    /// is not the device's own: a collection (`col-*`) holds nodes the
    /// device graph does not yet, so isolating them there finds nothing.
    static PREVIEW_GRAPH: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with previews resolving nodes in graph `gid` first.
pub fn with_preview_graph<T>(gid: Option<&str>, f: impl FnOnce() -> T) -> T {
    let prev = PREVIEW_GRAPH.with(|g| g.replace(gid.filter(|g| !g.is_empty()).map(str::to_string)));
    let out = f();
    PREVIEW_GRAPH.with(|g| *g.borrow_mut() = prev);
    out
}

/// The graph a preview of `device` reads: the open graph when one was named,
/// else the device's own, else a graph whose id is `device`.
fn preview_graph_at(root: &std::path::Path, device: &str) -> Option<metal_graph::Graph> {
    let hinted = PREVIEW_GRAPH.with(|g| g.borrow().clone());
    hinted
        .and_then(|gid| load_graph_at(root, &gid).ok())
        .filter(|g| g.domain == metal_graph::Domain::Live)
        .or_else(|| device_graph_at(root, device))
        .or_else(|| load_graph_at(root, device).ok())
}

/// A device's authored graph, if it has one and it is a Live graph.
///
/// Split out for O.4: the mode has to be read off this graph **before** the
/// layer list is resolved, and the plan is checked against it **after**, so
/// one frame needs it twice. Loading it once and passing it down is what
/// stops that from becoming two reads of the same file per device per frame.
pub fn device_graph_at(root: &std::path::Path, device_id: &str) -> Option<metal_graph::Graph> {
    let graph = load_graph_at(root, &device_graph_id(device_id)).ok()?;
    (graph.domain == metal_graph::Domain::Live).then_some(graph)
}

/// R22.6 decision 3 / Task #8: whether this device's graph declares its own
/// cut — at least one live `mask` node that names a device contour **or** a
/// 3D wall contour. When it does, the graph is the truth (regla 4) and the
/// device-wide `apply_masks` pass over `Output.masks` is skipped: the cut the
/// `mask` node writes (per-layer for 2D, whole-frame union for 3D, see
/// [`render_device`]) is the whole cut. A graph with no such `mask` node says
/// nothing, so the legacy whole-frame pass stands — zero migration for every
/// output that never drew one.
///
/// A wall with **no** contour is deliberately not a cut: the whole wall is the
/// whole surface, so there is nothing to cut away, and the source still draws
/// unwarped if the scene has no view or fit.
///
/// Bypassed `mask` nodes do not count: `apply_script_masks` already ignores
/// them, and a switched-off mask that still skipped the global pass would
/// leave the frame *uncut* where the operator expects the old behaviour.
pub fn graph_has_mask_node(root: &std::path::Path, device_id: &str) -> bool {
    device_graph_at(root, device_id).is_some_and(|g| {
        g.nodes.iter().any(|n| {
            n.kind == "mask"
                && !n.bypass
                && match mask_node_binding(n) {
                    Some(MaskBinding::Device(_)) => true,
                    Some(MaskBinding::Wall(w)) => w.contour.is_some(),
                    None => false,
                }
        })
    })
}

/// The device mode an authored graph declares, if it declares one — O.4's
/// `stack.mode` node, read through LAY.7b's `compose.*` family first.
///
/// # Why this is read here and not through `NodeLayerConfig`
///
/// `node_layer.rs` says why it stops short of `mode`, and it is right: a
/// `gen.*` node's config cannot decide which layers exist, because it is a
/// member of the very list that decision produces. The construct that *can*
/// is a node that is not part of the picture at all — today
/// `metal_graph::NodeFamily::Compose` — read one step earlier, which is what
/// [`compile_device_plan`] now does.
///
/// # The two vocabularies, in the order they are read
///
/// A `compose.*` node is the truth: its kind *is* the mode (`compose.layers`
/// is `Own`, and so on — `metal_graph`'s bridge table), and its config carries
/// only what the kind cannot, the collection id. The legacy `stack.mode` node
/// is read only when no compose node exists — a graph saved before the
/// migration, which the in-place conversion in
/// [`migrate_device_modes_to_graphs_at`] leaves no trace of once it runs.
/// Reading both, in that order, is what makes the change additive: an old file
/// keeps rendering, and a migrated one has one answer.
///
/// A node whose config does not parse, or a `compose.collection` with no id,
/// is treated as absent — the same "a malformed config must fall back, not
/// blank the screen" posture `NodeLayerConfig::from_node` takes, and here it
/// matters more: the fallback is the operator's own `stack.mode`, which is
/// never wrong, only older.
pub fn graph_device_mode(graph: &metal_graph::Graph) -> Option<layers::DeviceMode> {
    // R22.5b: the output node's own config is the truth now — the mode is a
    // property of `output.device`, not a second node. Read first, so a graph
    // written by the dropdown or a migrated one answers in one place.
    if let Some(mode) = output_device_mode(graph) {
        return Some(mode);
    }
    if let Some(node) = graph.nodes.iter().find(|n| metal_graph::is_compose_kind(&n.kind)) {
        let kind = metal_graph::mode_for_compose_kind(&node.kind)?;
        let id = if kind == "collection" {
            let cfg: serde_json::Value =
                serde_json::from_str(&node.config.to_json_str()).ok()?;
            Some(cfg.get("id")?.as_str()?.to_string())
        } else {
            None
        };
        return layers::DeviceMode::parse(kind, id.as_deref());
    }
    let node = graph
        .nodes
        .iter()
        .find(|n| n.kind == metal_graph::STACK_MODE_KIND)?;
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    let kind = cfg.get("kind")?.as_str()?;
    layers::DeviceMode::parse(kind, cfg.get("id").and_then(serde_json::Value::as_str))
}

/// R22.5b: the mode an `output.device` node declares in its own config —
/// `{"kind":"automix"|"collection"|"own", "id"?}` — the one place the mode
/// lives after this step. `None` for a node that declares none (every graph
/// authored before this), so [`graph_device_mode`] falls through to the
/// legacy `compose.*`/`stack.mode` read.
fn output_device_mode(graph: &metal_graph::Graph) -> Option<layers::DeviceMode> {
    let node = graph.nodes.iter().find(|n| n.kind == "output.device")?;
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    let mode = cfg.get("mode")?;
    let kind = mode.get("kind")?.as_str()?;
    layers::DeviceMode::parse(kind, mode.get("id").and_then(Value::as_str))
}

/// The node kind that makes a device automix. A `router` wired into the output
/// is the only thing that does: `output.device` never carries automix as a
/// parameter of its own. See [`resolve_authored_mode`].
pub const ROUTER_KIND: &str = "router";

/// Whether this graph holds a live `router` node — the automix trigger.
fn graph_has_router(graph: &metal_graph::Graph) -> bool {
    graph.nodes.iter().any(|n| n.kind == ROUTER_KIND && !n.bypass)
}

/// The mode an authored graph resolves to, for one frame. The single decision
/// behind `compile_device_plan_with`, so the rule lives in one place.
///
/// 1. A wired `router` means automix, whatever else the graph says.
/// 2. An explicit declared mode (output config, or a legacy compose/stack node)
///    stands, except a legacy `automix` on a canvas that wires a generator
///    straight into the output: that canvas is authored, so it is `Own`. The
///    stored automix was never the operator's choice on such a device.
/// 3. No declaration but a wired generator is an authored canvas: `Own`.
/// 4. Otherwise `None` — the caller falls back to the device's stored mode,
///    which keeps an empty graph drawing exactly what it drew before.
pub fn resolve_authored_mode(graph: &metal_graph::Graph) -> Option<layers::DeviceMode> {
    if graph_has_router(graph) {
        return Some(layers::DeviceMode::Automix);
    }
    let wired = !authored_generators_with_bindings(graph).is_empty();
    match graph_device_mode(graph) {
        Some(layers::DeviceMode::Automix) if wired => Some(layers::DeviceMode::Own),
        Some(mode) => Some(mode),
        None => wired.then_some(layers::DeviceMode::Own),
    }
}

/// Wires one `router` into the output's image port, if the graph has none.
/// The router has no inputs of its own: it is the automix switch, and the
/// layers come from the device's group, as they always did.
fn ensure_router_wired(graph: &mut metal_graph::Graph) {
    if graph.nodes.iter().any(|n| n.kind == ROUTER_KIND) {
        return;
    }
    let Some(out) = graph
        .nodes
        .iter()
        .find(|n| n.kind == "output.device")
        .map(|n| n.id.clone())
    else {
        return;
    };
    let router = graph.add_node("router", ROUTER_KIND, [600.0, 0.0]);
    graph.add_edge(
        metal_graph::Endpoint::new(&router, "image"),
        metal_graph::Endpoint::new(&out, "image"),
    );
}

/// Back-compat for the legacy `automix` mode (R22.5b), run on migrate. A
/// device whose output declares automix and wires no generator becomes the
/// explicit router. One that wires a generator keeps its authored canvas: the
/// declared automix is rewritten to `own`, so a later unwiring does not
/// resurrect it. Returns whether the graph changed.
fn migrate_legacy_automix(graph: &mut metal_graph::Graph) -> bool {
    if output_device_mode(graph) != Some(layers::DeviceMode::Automix) || graph_has_router(graph) {
        return false;
    }
    if authored_generators_with_bindings(graph).is_empty() {
        ensure_router_wired(graph);
    } else {
        let _ = set_graph_device_mode(graph, &layers::DeviceMode::Own);
    }
    true
}

/// Writes `mode` as `graph`'s one composition node — the single writer behind
/// the boot migration, `POST /layers { op: "mode" }` (the dropdown) and
/// `POST /graph { op: "mode" }` (the canvas).
pub fn set_graph_device_mode(
    graph: &mut metal_graph::Graph,
    mode: &layers::DeviceMode,
) -> Result<(), String> {
    set_graph_device_mode_at(graph, None, mode)
}

/// [`set_graph_device_mode`] with an explicit node to rewrite — the canvas op
/// points at a node and the write must land on *that* one, not on whichever
/// composition the graph happens to list first.
///
/// **One node, rewritten in place.** The target (or the first composition
/// node found — a `compose.*` one, or a legacy `stack.mode`) keeps its id and
/// becomes the mode's kind; any *other* composition node is retired, because
/// a graph with two is the ambiguous state `lint` refuses (the "all but one"
/// rule) and rewriting is the repair. `Disabled` retires the node and adds
/// nothing.
pub fn set_graph_device_mode_at(
    graph: &mut metal_graph::Graph,
    target: Option<&str>,
    mode: &layers::DeviceMode,
) -> Result<(), String> {
    // R22.5b: the mode lands on the `output.device` node's own config, merging
    // with whatever else that node holds. Every `compose.*`/`stack.mode` node
    // is retired in the same pass — the graph has one place for the mode now,
    // and a leftover second one would be the ambiguous state `lint` refuses.
    let out = match target {
        Some(id) if graph.node(id).is_some_and(|n| n.kind == "output.device") => Some(id.to_string()),
        // A canvas click on a legacy `compose.*`/`stack.mode` node still means
        // "this device's mode" — write it where it now lives.
        Some(_) | None => graph
            .nodes
            .iter()
            .find(|n| n.kind == "output.device")
            .map(|n| n.id.clone()),
    };
    if let Some(id) = &out {
        let node = graph.node(id).ok_or_else(|| format!("no such node: {id}"))?;
        let mut cfg: serde_json::Value =
            serde_json::from_str(&node.config.to_json_str()).unwrap_or_else(|_| json!({}));
        let mode_cfg = match mode {
            layers::DeviceMode::Collection(cid) => json!({ "kind": "collection", "id": cid }),
            layers::DeviceMode::Automix => json!({ "kind": "automix" }),
            layers::DeviceMode::Own => json!({ "kind": "own" }),
            layers::DeviceMode::Disabled => json!({ "kind": "disabled" }),
        };
        cfg["mode"] = mode_cfg;
        let node = graph.node_mut(id).expect("just read");
        node.config = metal_graph::Config::from_json_str(&cfg.to_string())
            .map_err(|e| e.to_string())?;
    }
    // The router is the automix switch and nothing else, so the mode writes it:
    // automix wires one in, every other mode takes any out again.
    if matches!(mode, layers::DeviceMode::Automix) {
        ensure_router_wired(graph);
    } else {
        // Only the routers feeding an output: one the operator placed elsewhere
        // stays.
        let routers: Vec<String> = graph
            .nodes
            .iter()
            .filter(|n| n.kind == ROUTER_KIND)
            .filter(|n| {
                graph.edges.iter().any(|e| {
                    e.from.node == n.id
                        && graph.node(&e.to.node).is_some_and(|t| t.kind == "output.device")
                })
            })
            .map(|n| n.id.clone())
            .collect();
        for id in routers {
            graph.remove_node(&id);
        }
    }
    let stale: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| metal_graph::is_compose_kind(&n.kind) || n.kind == metal_graph::STACK_MODE_KIND)
        .map(|n| n.id.clone())
        .collect();
    for id in stale {
        graph.remove_node(&id);
    }
    Ok(())
}

/// [`set_graph_device_mode`] against a device's own file. The dropdown's half
/// of "the graph is the truth": the route cannot leave the mode in a graph it
/// never wrote.
///
/// **A device with no graph is a no-op, not a canvas minted here.** The
/// canvas has exactly two creators — `ensure_device_graph_at` on `POST
/// /outputs {op:"add"}` and the boot migration — and a route that made a
/// third would be writing files a caller did not ask for (the test suite
/// pointed at the repo's own `data/` was the first symptom). A device with no
/// graph has no composition node, so `graph_device_mode` falls back to the
/// field, which the same route just wrote: the mode is not lost, and the next
/// boot materializes it into a canvas.
pub fn write_device_mode_at(
    root: &std::path::Path,
    device_id: &str,
    mode: &layers::DeviceMode,
) -> Result<(), String> {
    let gid = device_graph_id(device_id);
    let Ok(mut graph) = load_graph_at(root, &gid) else {
        return Ok(());
    };
    set_graph_device_mode(&mut graph, mode)?;
    save_graph_at(root, &graph)
}

/// The `source_id` a `gen.*` node names, if it names one — how a node says
/// "this member is that pool layer". Written by `metal_graph::migrate` and by
/// [`sync_device_graph_at`]; read by [`graph_composed_layers`].
fn source_id_of(node: &metal_graph::Node) -> Option<String> {
    if !node.kind.starts_with("gen.") {
        return None;
    }
    let cfg: Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    cfg.get("source_id")?.as_str().map(str::to_string)
}

/// The node whose `image` edge into `sink` is a member's membership edge: the
/// member itself, or the last `fx.*` of its chain when the canvas put one
/// there. `None` when the member does not reach the sink through a
/// single-output chain — the caller then wires the node straight in.
fn member_tail(
    graph: &metal_graph::Graph,
    from: &str,
    sink: &str,
) -> Option<metal_graph::NodeId> {
    let mut at = from.to_string();
    for _ in 0..=graph.nodes.len() {
        let next: Vec<&metal_graph::Edge> = graph
            .edges
            .iter()
            .filter(|e| e.from.node == at && e.from.port == "image")
            .collect();
        if next.len() != 1 {
            return None;
        }
        let to = next[0].to.node.clone();
        if to == sink {
            return Some(at);
        }
        if !graph.node(&to)?.kind.starts_with("fx.") {
            return None;
        }
        at = to;
    }
    None
}

/// The config [`sync_device_graph_at`] writes on a member node: the pool
/// layer's id plus the five fields [`node_layer::NodeLayerConfig`] reads, so
/// the node names its content *and* pins the same blend/z/sector/mask/opacity
/// the table holds. Everything else (effects, feed, library, active/solo)
/// stays the pool layer's own — the node is a pointer, not a copy.
fn layer_node_config(layer: &Layer) -> metal_graph::Config {
    let cfg = json!({
        "source_id": layer.id,
        "z": layer.z,
        "blend": layer.blend,
        "sector": layer.sector,
        "opacity": layer.opacity,
        "mask": layer.mask,
        "shader": layer.shader,
    });
    metal_graph::Config::from_json_str(&cfg.to_string()).expect("json! always serializes")
}

/// LAY.8c: the layer table's writer. Keeps a device's own graph in step with
/// the list the `/layers` routes just edited, so the graph is the origin of an
/// `Own` device's composition instead of a per-frame synthesis of the pool —
/// the last reconciliation between the table and the canvas.
///
/// **One `gen.*` node per layer, keyed by `source_id`.** A node naming a layer
/// still routed here is updated in place — kind, label, config — never
/// re-minted: `NodeId`s are append-only identity (an edge or a `cam.track`
/// association written against `gen-7` must keep meaning `gen-7`), so a
/// wholesale rebuild would churn the ids under the operator's canvas. A
/// managed node whose layer is no longer routed here is retired with its
/// edges; a canvas-only member (no `source_id`) is not managed and is never
/// touched.
///
/// **The order is the table's, and it survives the file.** Draw order is edge
/// order, and [`metal_graph::Graph::to_json`] sorts edges by id — with
/// [`metal_graph::IdGen`]'s unpadded `edge-10` sorting before `edge-2`, so the
/// writer assigns the membership edges zero-padded ids (`edge-0001`…) in draw
/// order. The table arrives z-ordered ([`layers::layers_on`]); this is what
/// makes "raise one layer" survive a save and a reload.
///
/// **The sink is wherever the graph already mixes.** If a `compose.*` node
/// feeds `output.device` — LAY.7c's wired mixer — members are wired into the
/// mixer's `image` port, through each member's own `fx.*` chain when it has
/// one: the chain is the operator's canvas work and is never rebuilt, only
/// re-connected to the sink. Otherwise members wire straight into the output.
/// The writer never restructures the graph's shape.
///
/// **A device with no graph is a no-op, not a canvas minted here** — the same
/// rule as [`write_device_mode_at`], and for the same reason: the canvas has
/// exactly two creators (`ensure_device_graph_at` on `POST /outputs {op:"add"}`
/// and the boot migration), and a route must not write files a caller did not
/// ask for. [`sync_device_graphs_once`] is what materializes an existing
/// install's lists, once, before the first frame.
///
/// **Idempotent**: the file is written only when its serialized form changes,
/// so a boot with nothing new leaves every device graph byte-identical.
pub fn sync_device_graph_at(
    root: &std::path::Path,
    device: &str,
    mode: Option<&layers::DeviceMode>,
    layers: &[&Layer],
    manifest_ids: &[&str],
) -> Result<(), String> {
    let gid = device_graph_id(device);
    let Ok(mut graph) = load_graph_at(root, &gid) else {
        return Ok(());
    };
    let Some(out_id) = graph
        .nodes
        .iter()
        .find(|n| n.kind == "output.device")
        .map(|n| n.id.clone())
    else {
        return Ok(());
    };
    let before = graph.to_json();
    // R22.5b: the mode lives on `output.device` now, so the sink is always the
    // output itself. A `compose.*`/`stack.mode` node an older writer left
    // behind has its members re-cabled straight onto `output.device.image`
    // (fan-in) before `set_graph_device_mode` retires it below.
    let sink = out_id.clone();
    let legacy_mixers: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| metal_graph::is_compose_kind(&n.kind) || n.kind == metal_graph::STACK_MODE_KIND)
        .map(|n| n.id.clone())
        .collect();
    for mixer in &legacy_mixers {
        let sources: Vec<(String, String)> = graph
            .edges
            .iter()
            .filter(|e| e.to.node == *mixer && e.to.port == "image")
            .map(|e| (e.from.node.clone(), e.from.port.clone()))
            .collect();
        let feeds_out = graph
            .edges
            .iter()
            .any(|e| e.from.node == *mixer && e.to.node == sink);
        if feeds_out {
            for (from_node, from_port) in sources {
                let already = graph.edges.iter().any(|e| {
                    e.from.node == from_node && e.to.node == sink && e.to.port == "image"
                });
                if !already {
                    graph.add_edge(
                        metal_graph::Endpoint::new(&from_node, &from_port),
                        metal_graph::Endpoint::new(&sink, "image"),
                    );
                }
            }
        }
    }

    // Which managed node each layer lands on, in table order. A layer with no
    // node yet is a member the table just added.
    let mut wanted: Vec<(String, metal_graph::NodeId)> = Vec::with_capacity(layers.len());
    for layer in layers {
        let found = graph
            .nodes
            .iter()
            .find(|n| source_id_of(n).as_deref() == Some(layer.id.as_str()))
            .map(|n| n.id.clone());
        let node_id = match found {
            Some(id) => id,
            None => {
                // A member the table just added reacts the way its layer did:
                // wired to the graph's audio node (plan "Audio input is a node").
                let id = graph.add_node("gen", "gen.", [0.0, 0.0]);
                metal_graph::migrate::wire_to_audio(&mut graph, &id);
                id
            }
        };
        let model_id = manifest_ids.get(layer.model).copied().unwrap_or("");
        let node = graph.node_mut(&node_id).expect("just found or added");
        node.kind = format!("gen.{model_id}");
        node.label = layer.name.clone();
        // The source's own reaction dials are not a layer field: keep them.
        node.config = layer_node_config(layer).carrying(&node.config, drive::CONFIG_KEY);
        wanted.push((layer.id.clone(), node_id));
    }

    // Retire the managed nodes whose layer left this device.
    let stale: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| {
            source_id_of(n)
                .map(|sid| !wanted.iter().any(|(lid, _)| *lid == sid))
                .unwrap_or(false)
        })
        .map(|n| n.id.clone())
        .collect();
    for id in stale {
        graph.remove_node(&id);
    }

    // Rebuild the membership edges in table order. Only the edge into the sink
    // is the writer's; each member's chain upstream of it stays where it is.
    let tails: Vec<metal_graph::NodeId> = wanted
        .iter()
        .map(|(_, id)| member_tail(&graph, id, &sink).unwrap_or_else(|| id.clone()))
        .collect();
    let managed: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| source_id_of(n).is_some())
        .map(|n| n.id.clone())
        .collect();
    graph.edges.retain(|e| {
        e.to.node != sink
            || !(managed.contains(&e.from.node) || tails.contains(&e.from.node))
    });
    for (i, tail) in tails.iter().enumerate() {
        graph.edges.push(metal_graph::Edge::new(
            format!("edge-{:04}", i + 1),
            metal_graph::Endpoint::new(tail, "image"),
            metal_graph::Endpoint::new(&sink, "image"),
        ));
    }

    if let Some(mode) = mode {
        set_graph_device_mode(&mut graph, mode)?;
    }
    if graph.to_json() == before {
        return Ok(());
    }
    save_graph_at(root, &graph)
}

/// The boot half of [`sync_device_graph_at`]: every device the app knows
/// about, with the list its pool currently routes to it. Runs right after
/// [`migrate_device_modes_to_graphs_once`], which is what creates the canvases
/// this fills — the mode pass first, then the members, so an install upgrading
/// into LAY.8c wakes up with its table already written as its graph.
pub fn sync_device_graphs_once(state: &State, manifest_ids: &[&str]) {
    let root = output::data_root();
    for id in state.device_ids() {
        let layers = state.layers_on(&id);
        if let Err(e) = sync_device_graph_at(&root, &id, None, &layers, manifest_ids) {
            eprintln!("  boot: cannot sync the layers of {id} into its graph: {e}");
        }
    }
}

pub fn authored_device_plan_from(
    graph: metal_graph::Graph,
    list: &[Layer],
    air: &layers::Airing,
    models: usize,
    manifest_ids: &[&str],
) -> Option<(
    Vec<Layer>,
    metal_graph::Graph,
    metal_graph::Plan,
    Vec<metal_graph::NodeId>,
)> {
    let gens_with = authored_generators_with_bindings(&graph);
    if gens_with.len() != list.len() {
        return None;
    }
    let gens: Vec<metal_graph::NodeId> = gens_with.iter().map(|(id, _, _)| id.clone()).collect();
    // Every branch must name the model that layer is actually showing. This
    // is the check that makes a stale graph fall back instead of drawing the
    // wrong model: `showing` moves with automix and with the router, and a
    // graph authored against yesterday's stack would otherwise silently
    // render whatever it was drawn with.
    for (layer, gen) in list.iter().zip(gens.iter()) {
        let want = manifest_ids
            .get(layer.showing(air, models))
            .copied()
            .unwrap_or("");
        if graph.node(gen)?.kind != format!("gen.{want}") {
            return None;
        }
    }
    // B3 paso 4: each generator may pin blend/z/sector/mask/opacity on the
    // layer it corresponds to — `gens[i]` is the node for `list[i]`, the same
    // zip the model check above just walked. A node with no config (or one
    // that fails to parse as a `NodeLayerConfig`) overrides nothing, so this
    // loop is a no-op for every graph authored before this existed.
    //
    // R22.6 / Task #8: a `mask` node between the branch and the device binds
    // that layer regardless — applied after the node's own config, the same
    // precedence `graph_composed_layers` uses.
    let mut list: Vec<Layer> = list.to_vec();
    for ((layer, gen), (_, binding, surface)) in
        list.iter_mut().zip(gens.iter()).zip(gens_with.iter())
    {
        if let Some(node) = graph.node(gen) {
            node_layer::NodeLayerConfig::from_node(node).apply_to(layer);
        }
        apply_binding(layer, binding.as_ref(), surface.as_ref());
    }
    let plan = graph.compile();
    Some((list, graph, plan, gens))
}

/// S.R mitad A's single compose-locality rule: a device with no `Output`
/// entry (the preview) has nowhere else to defer to and always composes
/// here; otherwise it composes here iff [`Output::composer`] names `local`.
pub fn composes_here(outputs: &Outputs, device: &str, local: &str) -> bool {
    match outputs.find(device) {
        Some(out) => out.composer(local) == local,
        None => true,
    }
}

/// Whether `device` has been handed to another program (`POST /outputs
/// {op:"cede"}`) — in which case this machine renders black and `GET
/// /frame` refuses with 409, the same posture as a device nobody
/// composes here.
pub fn is_ceded(outputs: &Outputs, device: &str) -> bool {
    outputs
        .find(device)
        .is_some_and(|out| out.ceded_to.is_some())
}

/// Builds the ordered layer list and the compiled [`metal_graph::Plan`] for a
/// device's current stack, deduplicating layers that would render identical
/// pixels onto one shared `gen.*` node.
///
/// **The only place [`layers::layers_on`] is called from, on the render path.**
/// Before this existed, both the old per-frame walk and the plan-based
/// prototype called it inline, independently — two call sites that could
/// only stay in agreement by discipline, not by construction. Moving it
/// here means sequencing is decided once, by the compiler step, and every
/// caller (today, [`render_device`]; the editor's own preview, eventually)
/// reads it off the plan rather than deciding it again.
///
/// Returns `None` for a disabled device or an empty stack — [`black`] is
/// what every caller wants for both, so the caller decides the frame size
/// rather than this function needing one just to hand back a colour.
///
/// **`playing` is applied to the list before a single key is computed, and
/// that ordering is the whole reason it is a parameter here rather than
/// something the caller does afterwards.** [`GenKey`] folds two layers onto
/// one `gen.*` node when they would draw the same pixels, and a layer's float
/// row is part of that judgement. An overlay changes exactly that row. Applied
/// later, two layers with the same model and the same absent base — the
/// ordinary fan-out case this function exists for — would share a node while
/// one of them was playing a movement and the other was not, and whichever row
/// was inserted into `rows` second would be drawn for both. Silent, in range,
/// and wrong (§3.3, §11.7). Applied here, the key sees the played row and the
/// two simply do not fold.
///
/// `device_id`/`all_layers` are new for the design notes PARTE IV §17.1/§17.4: `stack`
/// no longer carries its own layers, so this needs the pool and which device
/// it is compiling for, to filter it the way `stack.draw_order()` used to
/// filter `stack.layers`.
///
/// `composed_here` is S.R's compose-locality gate ([`composes_here`]):
/// `false` returns `None` before the authored graph is even loaded, because
/// a device this machine does not compose renders black here — no
/// fallback, no local compose for a screen it only shows.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn compile_device_plan(
    stack: &layers::Device,
    composed_here: bool,
    device_id: &str,
    all_layers: &[Layer],
    air: &layers::Airing,
    group: &[usize],
    energy0: f32,
    manifest_ids: &[&str],
    models: usize,
    playing: overlay::Playing<'_>,
    collections: &Collections,
    sequenced: Option<&str>,
    local: &str,
) -> Option<(
    Vec<Layer>,
    metal_graph::Graph,
    metal_graph::Plan,
    Vec<metal_graph::NodeId>,
    Vec<RemoteLayer>,
    layers::DeviceMode,
)> {
    compile_device_plan_with(
        stack, composed_here, device_id, all_layers, air, group, energy0, manifest_ids, models,
        playing, collections, sequenced, local, &|_| {},
    )
}

/// [`compile_device_plan`], with `lift` applied to this frame's copy of the
/// device graph right after it is loaded — Parity P-E's deferred bypass
/// (`transition::Toggles::lift`) un-bypasses a node still playing its
/// out-transition here, never in the file.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn compile_device_plan_with(
    stack: &layers::Device,
    composed_here: bool,
    device_id: &str,
    all_layers: &[Layer],
    air: &layers::Airing,
    group: &[usize],
    energy0: f32,
    manifest_ids: &[&str],
    models: usize,
    playing: overlay::Playing<'_>,
    collections: &Collections,
    // the design notes PARTE IV §17.5, second step: the collection a running
    // `sequence::Run` forces onto this device right now, if any — computed
    // once by the caller (`render_device`) so this and the composition
    // lookup there can never disagree about which collection is live. `None`
    // for "nothing scheduled", which is the overwhelming common case and
    // falls straight through to `stack.mode` below, unchanged.
    sequenced: Option<&str>,
    // The machine compiling this plan right now — the same name
    // `Output.node`/`composed_on` use. A layer whose `render_on` pins it to
    // some other name is excluded from the returned plan and reported in
    // the returned `remote` list instead — S.R punto 5's frame-shipping is
    // not built yet, so nothing consumes that list today.
    local: &str,
    lift: &dyn Fn(&mut metal_graph::Graph),
) -> Option<(
    Vec<Layer>,
    metal_graph::Graph,
    metal_graph::Plan,
    Vec<metal_graph::NodeId>,
    Vec<RemoteLayer>,
    // LAY.7b: the mode this frame actually resolved with, read off the graph
    // (or the field, for a device with no graph) — returned so the caller's
    // composition lookup cannot be a second, hand-kept-in-step read of the
    // same decision. See `render_device`.
    layers::DeviceMode,
)> {
    // S.R mitad A point 4: a device compiles only on the machine that
    // composes it — checked before anything else is loaded, since none of
    // it matters on a machine that will not draw this device at all.
    if !composed_here {
        return None;
    }
    // Loaded here rather than inside `authored_device_plan` because the mode
    // has to be known *before* the list exists — that ordering is the entire
    // reason `mode` could not be a `gen.*` node's config, and the reason
    // `#legacyTransmision` outlived every other part of the old transmission
    // panel. See `graph_device_mode`.
    let authored = device_graph_at(&output::data_root(), device_id).map(|mut g| {
        lift(&mut g);
        g
    });
    // `S.O.1`: a bypassed `output.device` is a screen switched off — the
    // half of blackout `POST /overlay/blackout` writes onto the graph
    // (O.28 punto 5). Read fresh off the graph just loaded, every frame
    // (regla 4), and answered the same way `Disabled` is, before any layer
    // is resolved: black at zero GPU cost. See `overlay::screen_off`.
    if authored.as_ref().is_some_and(overlay::screen_off) {
        return None;
    }
    // The graph wins when it says anything, because *"the config is a
    // reading of the graph, never a second state"* — a device whose graph
    // declares a composition has that composition, and the dropdown is a view
    // of it. A graph that declares none says nothing, so `stack.mode` stands,
    // which is every device authored before LAY.7b.
    // A graph that declares no mode but wires a generator into its output is
    // an authored canvas: it composes itself rather than falling to the
    // stored mode, whose default (automix) would ignore it entirely.
    let mode = authored
        .as_ref()
        .and_then(resolve_authored_mode)
        .unwrap_or_else(|| stack.mode.clone());
    // LAY.7b: for `compose.layers` — an Own device whose composition the
    // graph itself declares — the graph's own `gen.*` nodes are the list,
    // when it has any. This is the line that makes the graph the *origin* of
    // the composition rather than a coverage-checked overlay: add, remove or
    // rewire a generator in the canvas and the frame changes, with the pool
    // as the content store (`source_id`) rather than the member list.
    //
    // The pool path below still decides for every device whose graph wires
    // nothing — every migrated device, and automix/collection, which are
    // live reads by definition (§17.5).
    let graph_list = if matches!(mode, layers::DeviceMode::Own) {
        authored
            .as_ref()
            .and_then(|g| graph_composed_layers(g, all_layers, manifest_ids))
    } else {
        None
    };
    // A canvas that holds any `gen.*` node is being authored: it is the whole
    // picture, so nothing wired to the output means black, not the pool.
    let canvas_authored = matches!(mode, layers::DeviceMode::Own)
        && authored
            .as_ref()
            .is_some_and(|g| g.nodes.iter().any(|n| n.kind.starts_with("gen.")));
    if canvas_authored && graph_list.is_none() && sequenced.is_none() {
        return None;
    }
    let graph_composed = graph_list.is_some();
    let mut list: Vec<Layer> = match graph_list {
        Some(l) => l,
        None => match &mode {
        layers::DeviceMode::Disabled => return None,
        layers::DeviceMode::Automix => layers::automix(air.on_air, group, energy0),
        // the design notes PARTE IV §17.5: a device in `Collection(id)` mode reads
        // that collection live, every frame — selecting one is not copying
        // it (see the "mode" op), so this has to go back to
        // `state.collections` itself rather than to whatever this device's
        // own pool entries happen to hold. A missing id reads the same as
        // Disabled: nothing to draw, not a panic. `State::orphan` is what
        // normally keeps a device from pointing at a removed collection at
        // all; this is the fallback for the frame or two where it hasn't.
        layers::DeviceMode::Collection(id) => match collections.find(id) {
            Some(c) => c.layers.clone(),
            None => return None,
        },
        _ => layers::layers_on(all_layers, device_id)
            .into_iter()
            .cloned()
            .collect(),
        },
    };
    // the design notes PARTE IV §17.5, second step: a `sequence::Run` armed on this
    // device overrides whatever `stack.mode` just resolved, for as long as
    // one of its entries covers this instant — the same total-override
    // relationship `overlay::Overlays` has with a layer's own `base` row,
    // and for the same reason: loading a sequence onto a device is the
    // operator's action on *that* device, the same weight as clicking its
    // mode dropdown, just aimed ahead of time instead of live. A disabled
    // device already returned above, before this is ever consulted — turning
    // a screen off is the operator's explicit action on *that* screen, and a
    // schedule authored earlier must not be able to relight it in silence.
    if let Some(id) = sequenced {
        list = match collections.find(id) {
            Some(c) => c.layers.clone(),
            None => return None,
        };
    }
    if list.is_empty() {
        return None;
    }
    // A copy of the stack, so this writes a row into the frame about to be
    // drawn and never into the operator's saved layers.
    playing.apply(air, models, &mut list);

    // Remote layers leave the local plan but keep their place in the stack:
    // `pos` is how many local layers sit below them, so the compositor can
    // put their tile back at the same z (see `render_device_remote`).
    // The gen node each list entry came from, when the authored graph
    // composed the list (`graph_composed_layers` skips bypassed generators,
    // the same way `graph_composed` does here). A list the sequence or the
    // pool replaced has no such correspondence.
    let gen_of_entry: Vec<Option<metal_graph::NodeId>> = match authored.as_ref() {
        Some(g) if graph_composed && sequenced.is_none() => authored_generators_with_bindings(g)
            .into_iter()
            .map(|(id, _, _)| id)
            .filter(|id| !g.node(id).is_some_and(|n| n.bypass))
            .map(Some)
            .collect(),
        _ => Vec::new(),
    };
    let mut remote: Vec<RemoteLayer> = Vec::new();
    let mut local_only: Vec<Layer> = Vec::with_capacity(list.len());
    for (i, l) in std::mem::take(&mut list).into_iter().enumerate() {
        match &l.render_on {
            layers::RenderOn::Node(n) if n != local => remote.push(RemoteLayer {
                pos: local_only.len(),
                node: n.clone(),
                layer: l,
                gen: gen_of_entry.get(i).cloned().flatten(),
            }),
            _ => local_only.push(l),
        }
    }
    let mut list = local_only;
    // A device whose every layer is drawn elsewhere still composes: its
    // remote tiles are the picture, so only "nothing at all" is black.
    if list.is_empty() && remote.is_empty() {
        return None;
    }

    // Plan step 3. Everything above this line resolved *what is on this
    // device right now* — mode, sequence, overlays — and none of it changes
    // depending on whether a graph is attached. That is plan step 2's whole
    // content, and it is why the authored graph can be consulted here, once,
    // against a settled list, rather than having to reimplement automix and
    // overlays as node kinds first.
    //
    // Synthesis below is the fallback for a device whose graph wires
    // nothing: it builds the same one-generator-per-unique-`GenKey` shape the
    // canvas would, and the golden corpus pins the frames it produces.
    if let Some(graph) = authored {
        // A graph-wired composition already *is* the list — built from this
        // graph's own nodes above — so there is nothing to check it against.
        // The coverage rule `authored_device_plan_from` applies exists to
        // decide whether a graph may override a list resolved *elsewhere*;
        // here it did not come from elsewhere.
        if graph_composed {
            let node_of_layer = authored_generators(&graph);
            let plan = graph.compile();
            return Some((list, graph, plan, node_of_layer, remote, mode));
        }
        if let Some((list, graph, plan, node_of_layer)) =
            authored_device_plan_from(graph, &list, air, models, manifest_ids)
        {
            return Some((list, graph, plan, node_of_layer, remote.clone(), mode));
        }
    }

    let mut graph = metal_graph::Graph::new("device", "device", metal_graph::Domain::Live);
    let out_node = graph.add_node("out", "output.device", [0.0, 0.0]);
    let mut node_of_layer: Vec<metal_graph::NodeId> = Vec::with_capacity(list.len());
    // Which node each already-seen key landed on — linear rather than a
    // `HashMap`, because `Limits::max_layers` bounds this at sixteen and a
    // `GenKey` (a `Vec<f32>` inside) has no clean hash anyway.
    let mut seen: Vec<(GenKey, metal_graph::NodeId)> = Vec::new();
    for layer in &list {
        let showing = layer.showing(air, models);
        let key = GenKey {
            showing,
            base: layer.base.clone(),
            library: layer.library.clone(),
            effects: layer.effects.clone(),
            fijo_mask: layer.fijo_mask,
            fondo_alpha: layer.fondo_alpha,
            shader: layer.shader.clone(),
        };
        let gen_id = match seen.iter().find(|(k, _)| *k == key) {
            Some((_, id)) => id.clone(),
            None => {
                let model_id = manifest_ids.get(showing).copied().unwrap_or("");
                let id = graph.add_node("gen", &format!("gen.{model_id}"), [0.0, 0.0]);
                // The one thing a synthesized node has to carry: which WGSL
                // a `forge` source draws (`Paint::run` reads it off here).
                if let Some(shader) = &key.shader {
                    if let Some(node) = graph.node_mut(&id) {
                        node.config = crate::node_layer::NodeLayerConfig {
                            shader: Some(shader.clone()),
                            ..Default::default()
                        }
                        .to_config();
                    }
                }
                seen.push((key, id.clone()));
                id
            }
        };
        graph.add_edge(
            metal_graph::Endpoint::new(&gen_id, "image"),
            metal_graph::Endpoint::new(&out_node, "image"),
        );
        node_of_layer.push(gen_id);
    }
    // Plan "Audio input is a node": a synthesized graph says where its
    // sources listen, the same as a migrated one — one `audio.live` with a
    // `null` source (the seed) wired to every generator's `control`.
    let audio = graph.add_node("audio", crate::listeners::AUDIO_LIVE, [-200.0, 0.0]);
    for (_, gen_id) in &seen {
        graph.add_edge(
            metal_graph::Endpoint::new(&audio, "control"),
            metal_graph::Endpoint::new(gen_id, "control"),
        );
    }
    let plan = graph.compile();
    Some((list, graph, plan, node_of_layer, remote, mode))
}

/// The part of a rendered frame a [`Layer::span`] window names, as its own
/// RGBA buffer — or `None` when the window is the whole frame (nothing to
/// copy) or degenerate. The window is clamped to the frame and never
/// narrower than one pixel, so a window slightly off the edge still shows
/// the pixels it does cover.
pub fn crop_window(px: &[u8], w: u32, h: u32, win: &Sector) -> Option<(Vec<u8>, u32, u32)> {
    let (x0, y0, cw, ch) = crop_rect(w, h, win)?;
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in y0..y0 + ch {
        let start = ((y * w + x0) * 4) as usize;
        out.extend_from_slice(&px[start..start + (cw * 4) as usize]);
    }
    Some((out, cw, ch))
}

/// The pixel rectangle `(x, y, w, h)` of a `w x h` frame a window names (see
/// [`crop_window`]); `None` when it is the whole frame or degenerate.
pub fn crop_rect(w: u32, h: u32, win: &Sector) -> Option<(u32, u32, u32, u32)> {
    if w == 0 || h == 0 || !(win.w > 0.0 && win.h > 0.0) {
        return None;
    }
    let x0 = (win.x.clamp(0.0, 1.0) * w as f32).floor() as u32;
    let y0 = (win.y.clamp(0.0, 1.0) * h as f32).floor() as u32;
    let x1 = (((win.x + win.w).clamp(0.0, 1.0) * w as f32).ceil() as u32).clamp(x0 + 1, w);
    let y1 = (((win.y + win.h).clamp(0.0, 1.0) * h as f32).ceil() as u32).clamp(y0 + 1, h);
    if x0 >= w || y0 >= h || (x0, y0, x1, y1) == (0, 0, w, h) {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

/// The clock the native script presets read: the musical beat and show time.
fn script_clock(state: &State, time: f32) -> crate::script_preset::BeatClock {
    crate::script_preset::BeatClock { beats: state.music_pos.beats(), time, bpm: state.music.bpm() }
}

/// 2026-10-01: hands every `reel.batch` of this frame's graph the input's beat
/// count ([`crate::reel_batch::beat_clock`]), for a batch stepping in beats.
/// Only the in-memory copy is touched.
fn stamp_beats(graph: &mut metal_graph::Graph, sim: &Input, time: f32) {
    if !graph.nodes.iter().any(|n| n.kind == crate::reel_batch::REEL_BATCH_KIND) {
        return;
    }
    let (beats, spb) = crate::reel_batch::beat_clock(sim, time);
    crate::reel_batch::stamp(graph, beats, spb);
}

/// Composes one device's stack into a frame.
///
/// The one path both `/preview.jpg` and `/output.jpg` take, because the preview
/// showing something other than what a screen shows is the failure that makes a
/// preview worse than no preview.
///
/// **This is the plan-based path, live since the swap `the design notes`
/// §10 Conjunto B's own gate asked for.** [`compile_device_plan`] builds the
/// graph (and is the only caller of `Device::draw_order` — B3), rendering
/// runs through [`Paint::run`] (B1) instead of looping `Paint::solo` per
/// layer, and the pixel budget in [`compose::plan_dedup`] (B5) charges a
/// shared generator once. The per-layer walk this replaced is deleted; the
/// corpus in this file's tests pins the frames it used to be diffed against.
/// The bounding box, in device 0..1, of the surface's projection through `h`:
/// `[x, y, w, h]`, or `None` when every probe point folds behind the
/// projector. The four surface corners and the centre are what a homography
/// can put in front; a point that folds is skipped rather than dragging the
/// box across infinity.
fn projected_bbox(h: &escena::Homography) -> Option<[f32; 4]> {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    let mut any = false;
    for (s, t) in [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.5)] {
        if let Some(p) = h.map([s, t]) {
            any = true;
            lo[0] = lo[0].min(p[0]);
            lo[1] = lo[1].min(p[1]);
            hi[0] = hi[0].max(p[0]);
            hi[1] = hi[1].max(p[1]);
        }
    }
    any.then(|| [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]])
}

/// P2: where a source pinned to a surface is composited — the projected
/// contour's bounding box in device 0..1, clamped to the device. The warp
/// decides which pixels inside it land; outside it nothing is walked, so the
/// source does not smear past the surface's own outline.
fn surface_place(h: &escena::Homography) -> Sector {
    place_from_bbox(projected_bbox(h))
}

/// A device-space bbox as a compositing sector, clamped to the frame. `None`
/// (nothing in front of the projector, or an empty mesh) is a zero sector: the
/// old whole-frame fallback smeared the source across the output, so such a
/// layer is skipped instead.
fn place_from_bbox(bbox: Option<[f32; 4]>) -> Sector {
    match bbox {
        Some([x, y, w, hh]) => Sector { x, y, w, h: hh }.clamped(),
        None => Sector { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
    }
}

/// P-V: the place of a layer dealt to one sector — the polygon's own
/// bounding box in device `0..1`.
fn sector_place(poly: &[[f32; 2]]) -> Sector {
    place_from_bbox(bbox_of(poly))
}

/// The `[x, y, w, h]` bounding box of a device-space polygon, clamped to the
/// frame; `None` for a polygon with no points.
fn bbox_of(poly: &[[f32; 2]]) -> Option<[f32; 4]> {
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for p in poly {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    (hi[0] > lo[0] && hi[1] > lo[1]).then(|| [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]])
}

/// P3: the place of a mesh-warped layer, from the mesh's own device bbox.
fn mesh_place(mesh: &compose::WarpMesh) -> Sector {
    place_from_bbox(mesh.bbox())
}

/// The bbox a surface-bound layer is drawn into: the mesh's when it has one,
/// otherwise its homography's projected bbox. This is what lets a mesh-warped
/// layer still get a sector and a render size.
fn warp_bbox(warp: Option<&compose::Warp>, mesh: Option<&compose::WarpMesh>) -> Option<[f32; 4]> {
    match mesh {
        Some(mesh) => mesh.bbox(),
        None => warp.and_then(|w| projected_bbox(&w.homography)),
    }
}

/// P2 item 3: the size a surface-pinned source *renders* at — the projected
/// contour's bounding box × 1.5, in device pixels, not the device size. The
/// margin covers the warp's bilinear sampling; a patch of wall does not need a
/// device-sized buffer.
fn surface_render_size(h: &escena::Homography, width: u32, height: u32) -> (u32, u32) {
    render_size_from_bbox(projected_bbox(h), width, height)
}

/// The render size for a device-space bbox: bbox × 1.5 so the warp's bilinear
/// sampling has a margin, in device pixels.
fn render_size_from_bbox(bbox: Option<[f32; 4]>, width: u32, height: u32) -> (u32, u32) {
    let (bw, bh) = match bbox {
        Some([_, _, w, hh]) => (w * 1.5, hh * 1.5),
        None => (1.0, 1.0),
    };
    let (_, _, pw, ph) = Sector { x: 0.0, y: 0.0, w: bw, h: bh }.clamped().pixels(width, height);
    (pw, ph)
}

/// P3: the render size of a mesh-warped layer, from the mesh's device bbox.
fn mesh_render_size(mesh: &compose::WarpMesh, width: u32, height: u32) -> (u32, u32) {
    render_size_from_bbox(mesh.bbox(), width, height)
}

/// P3 item 3: the size a spanned source *renders* at, once, instead of at each
/// device's own size. The windows in `spans` are `0..1` of the picture —
/// `Venue::spans` normalises them to the union box around every display — so a
/// window a fraction `f` wide crops `f × canvas` pixels. Rendering the whole
/// picture at the size the *smallest* window needs (`device / f`, the covering
/// canvas) leaves that window native and every larger one supersampled, so no
/// device shows an upscale. A layer with no window keeps the device size.
fn span_canvas_size(
    spans: &std::collections::BTreeMap<String, Sector>,
    width: u32,
    height: u32,
) -> (u32, u32) {
    let mut sx = 1.0f32;
    let mut sy = 1.0f32;
    for win in spans.values() {
        if win.w > 0.0 {
            sx = sx.max(1.0 / win.w);
        }
        if win.h > 0.0 {
            sy = sy.max(1.0 / win.h);
        }
    }
    (
        ((width as f32 * sx).round() as u32).max(1),
        ((height as f32 * sy).round() as u32).max(1),
    )
}

/// A layer another machine draws (`render_on = { node }` naming it). It is
/// kept out of the local plan; `pos` is how many local layers sit below it in
/// the stack, which is where its tile goes in the composition.
#[derive(Clone)]
pub struct RemoteLayer {
    pub pos: usize,
    pub node: String,
    pub layer: Layer,
    /// The authored `gen.*` node this layer is, when the device's graph
    /// composes it. Its `fx.*` chain is resolved from that node, as a local
    /// layer's is. `None` for a synthesized graph, which has no chain.
    pub gen: Option<metal_graph::NodeId>,
}

/// What a remote layer must be drawn from, resolved here on the requesting
/// machine: the model its feed picked this frame, the float row (base floats
/// with the audio digest and post effects already folded in), the draw time,
/// and the size the tile is composed at.
pub struct RemoteAsk<'a> {
    pub layer: &'a Layer,
    pub node: &'a str,
    pub model: usize,
    pub row: &'a [f32],
    /// The resolved `fx.*` chain the peer applies after the generator, in
    /// graph order — the same steps `Paint::run` hands `solo_chain` locally.
    pub chain: &'a [metal_schema::EffectStep],
    /// The draw time: the generator's own `fx.speed` clock when its graph
    /// node has one, the show time otherwise (what `Paint::run_with_clocks`
    /// hands `solo_chain` for a local generator).
    pub time: f32,
    /// The source's `fondo_alpha`, carried to the peer's `solo_chain`.
    pub fondo_alpha: bool,
    /// The generator clock's rate per show second (its `fx.speed` product,
    /// `1.0` when none). A prefetched frame advances `time` by this much of
    /// a frame, the way the local clock would.
    pub speed: f32,
    pub frame: u64,
    pub w: u32,
    pub h: u32,
}

/// Supplies one remote layer's picture as `(w, h, RGB)`, or `None` when it is
/// not available (the layer is then left out of the frame). Must not block:
/// the frame does not wait for a peer.
pub type RemoteFrames<'a> = dyn FnMut(&RemoteAsk) -> Option<(u32, u32, Vec<u8>)> + 'a;

/// [`crate::edge_blend::blend_map`], kept between frames per device and size
/// while the venue's projectors and surfaces stay the same.
fn cached_blend_map(v: &crate::venue::Venue, device: &str, w: u32, h: u32) -> Option<std::sync::Arc<crate::edge_blend::BlendMap>> {
    use std::sync::{Arc, Mutex, OnceLock};
    type Cache = HashMap<(String, String, u32, u32), (String, Option<Arc<crate::edge_blend::BlendMap>>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    if !v.projectors.iter().any(|p| p.blend.enabled) {
        return None;
    }
    let key = serde_json::to_string(&(&v.projectors, &v.surfaces)).unwrap_or_default();
    let slot = (v.venue.id.clone(), device.to_string(), w, h);
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    if let Some((k, map)) = cache.get(&slot) {
        if *k == key {
            return map.clone();
        }
    }
    let map = crate::edge_blend::blend_map(v, device, w, h).map(Arc::new);
    cache.insert(slot, (key, map.clone()));
    map
}

pub fn render_device(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    w: u32,
    h: u32,
) -> (Vec<u8>, u32, u32) {
    render_device_remote(paint, sim, state, device, w, h, &mut |_| None)
}

/// [`render_device`], with the device's remote layers (`render_on = node`
/// naming another machine) composed from `remote` at their own place in the
/// stack: same opacity, blend, sector, clip and giro as a local tile.
pub fn render_device_remote(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    w: u32,
    h: u32,
    remote: &mut RemoteFrames<'_>,
) -> (Vec<u8>, u32, u32) {
    match render_device_frame(paint, sim, state, device, w, h, remote) {
        DeviceFrame::Cpu(px, w, h) => (px, w, h),
        // The sink that wants bytes (this signature) pays the readback; a
        // caller that can take a texture asks `render_device_frame`.
        DeviceFrame::Gpu(f) => (paint.readback(&f), f.width(), f.height()),
    }
}

/// Where one layer's pixels are on their way into compose.
enum Px<'a> {
    Tex(&'a crate::gpu_frame::GpuFrame),
    Cpu(&'a [u8], u32, u32),
}

struct Entry<'a> {
    px: Px<'a>,
    layer: &'a Layer,
    place: Sector,
    clip: Option<&'a Mask>,
    warp: Option<compose::Warp>,
    /// P3: a non-planar surface's mesh warp. CPU-only for now; `None` on the
    /// homography path and for remote layers.
    mesh: Option<compose::WarpMesh>,
}

/// G1: a finished device frame — still a texture on the GPU backend when
/// nothing after compose needed CPU pixels, CPU bytes otherwise.
pub enum DeviceFrame {
    Cpu(Vec<u8>, u32, u32),
    Gpu(crate::gpu_frame::GpuFrame),
}

/// [`render_device_remote`] without the final readback. The window presents
/// from the texture; Art-Net, NDI, v4l2, PNG and peer sinks read it back
/// through [`Paint::readback`] / [`crate::gpu_compose::AsyncReadback`].
pub fn render_device_frame(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    w: u32,
    h: u32,
    remote: &mut RemoteFrames<'_>,
) -> DeviceFrame {
    // G3: degrade inside the GPU. A device larger than the adapter's texture
    // limit (or than memory allows) composes lower and is upscaled.
    let (cw, ch) = paint.fit_size(w, h);
    if (cw, ch) != (w, h) {
        let small = render_device_frame(paint, sim, state, device, cw, ch, remote);
        return upscale_device_frame(paint, small, w, h);
    }
    let time = state.time();
    // The frame the router saw this instant, via the one Looper — so the
    // picture and the routing decision are made from the same frame (plan
    // decision 5). `poll` is beat-phase indexed, so the several calls this
    // frame makes all return the same row.
    let frame = sim.clock_frame(&mut state.looper, time);
    let air = state.airing(sim, time);
    let group: Vec<usize> = state.router.group().to_vec();
    let limits = state.limits;
    let models = paint.model_count();

    let Some(stack) = state.stack(device) else {
        return DeviceFrame::Cpu(vec![0u8; (w * h * 4) as usize], w, h);
    };
    // S.AL.1: the head this device drives, if any — read here (while
    // `stack`'s own borrow is still alive) and kept as an owned value so it
    // survives to the end of the function, past every later use of `state`
    // as `&mut`. `None` for every device that is a screen, not a fixture.
    let head = stack.head.clone();
    // the design notes PARTE IV §17.5, second step: what a running `sequence::Run`
    // forces this device to show right now, if anything — asked once, here,
    // and handed to `compile_device_plan`, so the two can never resolve to
    // different collections. `Disabled` still wins over this:
    // `compile_device_plan` returns `None` for it before ever looking at
    // `sequenced`.
    let sequenced = state.runs.active(device, time);

    let manifest_ids = paint.ids();
    let bpm = state.music.bpm();
    let toggles = &state.transitions;
    let presets = &state.transition_presets;
    let lifted: std::cell::RefCell<Vec<String>> = Default::default();
    let lift = |g: &mut metal_graph::Graph| *lifted.borrow_mut() = toggles.lift(g, time, bpm, presets);
    let Some((mut list, graph, plan, node_of_layer, remote_layers, mode)) = compile_device_plan_with(
        stack,
        composes_here(&state.outputs, device, &state.local_node)
            && !is_ceded(&state.outputs, device),
        device,
        &state.layers,
        &air,
        &group,
        frame.f[0],
        &manifest_ids,
        models,
        state.overlays.on(device, time),
        &state.collections,
        sequenced,
        &state.local_node,
        &lift,
    ) else {
        // Disabled, or an empty stack — both read as a black frame at the
        // requested size, same as the legacy walk.
        let dark = black(w, h);
        // Frame-driven fixtures follow the frame here too (a blacked-out or
        // emptied screen darkens its lights) — unless another machine or
        // program composes this device, which then drives them itself.
        if composes_here(&state.outputs, device, &state.local_node)
            && !is_ceded(&state.outputs, device)
        {
            state.wire_errors += crate::wire::drive_device(
                &output::data_root(),
                device,
                None,
                &crate::wire::SourceRows::default(),
                &dark,
                w,
                h,
                &mut state.sampled_wires,
                &mut state.dmx_bus,
                web_time::Instant::now(),
            );
        }
        return DeviceFrame::Cpu(dark, w, h);
    };

    // the design notes PARTE IV §17.5: a device in `Collection(id)` mode shows that
    // collection live, so its composition has to come from the collection
    // too, not from `stack.composition` — which "apply" is now the only op
    // that ever writes, and which would otherwise freeze at whatever it was
    // when the device last held its own stack.
    //
    // Read off the mode `compile_device_plan` just resolved with (LAY.7b)
    // rather than off `stack.mode` a second time: one read of the decision,
    // so the composition and the layer list cannot disagree — and, since
    // LAY.7b, so both see a `compose.*` node the dropdown wrote.
    let composition = match sequenced.or(match &mode {
        layers::DeviceMode::Collection(id) => Some(id.as_str()),
        _ => None,
    }) {
        Some(id) => state
            .collections
            .find(id)
            .map(|c| c.composition)
            .unwrap_or(stack.composition),
        None => stack.composition,
    };

    // Where each source actually goes, and what shape it is when it gets there.
    // A source bound to a contour is placed at the contour's own bounding box
    // rather than at the rectangle in its record: that is the region the
    // polygon can use, and rendering a full frame to throw most of it away is
    // most of a GPU pass per source per frame.
    // The device's whole mask list travels alongside: since H.5b/H.10 a
    // contour may name a `parent` or be the boolean of two others, and
    // neither resolves from the mask on its own. A per-frame copy, and
    // `mut`: a `mask` node driven by a script (O.6/S.G, `apply_script_masks`
    // below) rewrites its contour *here*, in the frame about to be drawn,
    // never in `outputs.json` — the same rule `playing.apply` follows for a
    // layer's row.
    let mut device_masks: Vec<Mask> = state
        .outputs
        .find(device)
        .map(|o| o.masks.clone())
        .unwrap_or_default();
    let passage = state.router.content.passage();

    // One digest per audio node (plan "Audio input is a node"): each source
    // reads the frame of the audio node wired to its `control`, and a source
    // with nothing wired is static. The consumers keyed by graph (bindings,
    // scripts, raw-ingest effects, fixtures) read the graph's audio frame.
    let digests = sim.digests(&graph, frame, time);
    let frame = digests.graph_frame(&graph);

    let mut rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
        std::collections::HashMap::with_capacity(list.len());
    // Fixtures F4: the operator's own **base** row per generator, beside the
    // driven one, so a `venue.fixture` node with `hue_lock = "hold"` can read
    // the model's own colour slot off it (decision 6). Same source `row_for_node`
    // drives from, so the two cannot disagree about what "own" means.
    let mut base_rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
        std::collections::HashMap::with_capacity(list.len());
    // Mirrors `rows`: each layer's `fondo_alpha` (the design notes PARTE IV §17.3),
    // carried to the `Paint::solo` call `run` makes on its behalf.
    let mut fondo_alpha: std::collections::HashMap<metal_graph::NodeId, bool> =
        std::collections::HashMap::with_capacity(list.len());
    // A remote layer's generator (one the graph names) is driven here too, in
    // the same pass: its row is what the peer is asked to draw, so it must
    // carry the bindings and script outputs a local generator's row does.
    let driven: Vec<(&Layer, &metal_graph::NodeId)> = list
        .iter()
        .zip(node_of_layer.iter())
        .chain(remote_layers.iter().filter_map(|r| r.gen.as_ref().map(|g| (&r.layer, g))))
        .collect();
    for (layer, gen_id) in driven {
        let showing = layer.showing(&air, models);
        let base: Vec<f32> = layer
            .base
            .clone()
            .unwrap_or_else(|| state.base[showing.min(state.base.len() - 1)].clone());
        let wired = digests.wired(&graph, gen_id);
        let mut row = row_for_node(
            Some(&graph),
            Some(gen_id.as_str()),
            paint,
            state,
            showing,
            &base,
            wired,
            time,
            passage,
            layer.library.as_deref(),
        );
        // A `forge` source's own floats are the ingest as it arrives — the
        // shader was written against "rms in `floats2.x`", not against a
        // swing around a slider (`forge::ingest_row`).
        if manifest_ids.get(showing).copied() == Some(crate::forge::MODEL) {
            crate::forge::ingest_row(&mut row, wired.unwrap_or(&Digests::zero()));
        }
        apply_node_post(&mut row, &layer.effects, layer.fijo_mask);
        rows.insert(gen_id.clone(), row);
        base_rows.insert(gen_id.clone(), base);
        fondo_alpha.insert(gen_id.clone(), layer.fondo_alpha);
    }

    // O.6/S.G: the graph's `script.*` nodes run here, once per frame, after
    // the generators' rows exist (a script fed by a `gen.*` `row` port reads
    // exactly the row just driven above) and before anything downstream of
    // a script is resolved. `script::evaluate` reads each node's code and
    // floats off `Node.config` live (regla 4) and answers a row per node;
    // what those rows feed is decided by the graph's own edges, read below
    // by the one consumer that exists today — a `mask` node's `control`.
    //
    // Param edges (`crate::bind`) around it: held generator floats and every
    // binding a script does not produce land first, so a script reading a
    // generator row sees them; the scripts honour bindings into their own
    // floats; then every binding lands for good. Written into `rows` and into
    // this frame's copy of the graph (an effect's amount), never the file.
    let mut graph = graph;
    let gen_params = crate::bind::gen_params(&graph, &manifest_ids, &paint.schemas());
    // The Looper's two bindable floats for this frame (`control.repeat.locked`/
    // `.phase`). `status()` reads by value off a short immutable borrow, so it
    // does not outlive the call and cannot clash with the `&mut` uses below.
    let repeat = {
        let s = state.looper.status();
        crate::bind::RepeatVars { locked: if s.engaged.is_some() { 1.0 } else { 0.0 }, phase: s.phase }
    };
    // Effect loops (T8) land before the bindings, so a bound amount wins.
    apply_effect_loops(&mut graph, &mut rows, &state.loops, state.music_pos);
    crate::bind::before_scripts(&mut graph, &plan, &mut rows, &gen_params, &frame.raw, &repeat);
    let scripts = crate::script::evaluate_clocked(&graph, &rows, &frame.raw, &gen_params, script_clock(state, time));
    let script_rows = scripts.rows;
    crate::bind::after_scripts(&mut graph, &plan, &mut rows, &script_rows, &gen_params, &frame.raw, &repeat);
    // H3: `cam.track` sums its viewer offset into the reserved camera floats
    // of each target's row, on top of the sequence/`ai.*` values the drive
    // already wrote. Applied here so the live view and the render see it.
    // The sensors run first, so `state.head` holds this frame's reading.
    if graph.nodes.iter().any(|n| n.kind == "sensor.yolo") {
        poll_head_sensors(&graph, state, HEAD_DETECT_W, HEAD_DETECT_H);
    }
    if graph.nodes.iter().any(|n| n.kind == "cam.track") {
        apply_camera_track(&graph, &mut rows, shared_manifest(), &state.head);
    }
    state.live.record(&graph, &rows, &script_rows, &scripts.floats, &gen_params, &frame.raw, time);
    apply_script_masks(&graph, &script_rows, &mut device_masks);

    // Task #8: the 3D half. A layer bound to a wall is composited through this
    // output's homography for that wall, and cut to the contour the node
    // names. The scene is loaded once per frame and only when some layer
    // actually binds a wall, so the no-escena path reads nothing it did not
    // before and allocates nothing extra.
    // Remote layers take the same warp as local ones, so they count here too.
    let all_layers = || list.iter().chain(remote_layers.iter().map(|r| &r.layer));
    let has_wall = all_layers().any(|l| l.mask_wall.is_some());
    let scene = has_wall.then(|| escena::load(&output::data_root()));
    // Every wall contour this output can see, projected once. `project` is the
    // escena module's single projection seam; the 3D clip is one of these.
    let projected: Vec<Mask> = scene.as_ref().map(|e| e.project(device)).unwrap_or_default();

    // P2: the venues the layers pinned to a surface read. Loaded once per
    // distinct venue, and only when some layer actually binds a surface, so
    // the no-surface path reads nothing it did not before. An empty `venue`
    // on the binding (a migrated `mask`, whose `venue.surface` names only the
    // surface) reads the venue projected last, the same `venue::last` a
    // `look_venue` with no id falls back to.
    let root = output::data_root();
    let mut venues: HashMap<String, crate::venue::Venue> = HashMap::new();
    for layer in all_layers() {
        let Some(binding) = &layer.surface else { continue };
        if venues.contains_key(&binding.venue) {
            continue;
        }
        let name = if binding.venue.is_empty() {
            crate::venue::last(&root).unwrap_or_default()
        } else {
            binding.venue.clone()
        };
        if name.is_empty() {
            continue;
        }
        if let Ok(v) = crate::venue::load(&root, &name) {
            venues.insert(binding.venue.clone(), v);
        }
    }
    // The warp for each layer, parallel to `list`: `None` for every layer not
    // bound to a wall or a surface, and for one the venue cannot map (no
    // projector, no pins). A layer with no warp still draws — it is never
    // dropped, only left unwarped, which is exactly today's behaviour.
    let warp_of = |l: &Layer| -> (Option<compose::Warp>, Option<compose::WarpMesh>) {
        // P2: `venue.surface` wins over the older `mask` wall binding when
        // a branch somehow carries both — the surface is the newer, more
        // specific statement of where the picture lands.
        if let Some(binding) = &l.surface {
            let Some(v) = venues.get(&binding.venue) else {
                return (None, None);
            };
            // P3: a planar surface keeps the exact homography path; anything
            // else gets the tessellated mesh. A surface with neither is
            // unmappable, and the P2.4 skip below drops it.
            return match v.surface_homography(device, &binding.surface) {
                Some(h) => (Some(compose::Warp::new(h)), None),
                None => (None, v.warp_mesh(device, &binding.surface, 24, 12)),
            };
        }
        let Some(binding) = l.mask_wall.as_ref() else {
            return (None, None);
        };
        let Some(scene) = scene.as_ref() else {
            return (None, None);
        };
        let Some(wall) = scene.wall(&binding.wall) else {
            return (None, None);
        };
        (scene.homography(device, wall).map(compose::Warp::new), None)
    };
    let layer_warps: Vec<(Option<compose::Warp>, Option<compose::WarpMesh>)> =
        list.iter().map(|l| warp_of(l)).collect();

    // P-V: the sector polygon each surface-bound layer lands in, in device
    // `0..1` — `None` for a layer not on a surface, and for one the venue
    // cannot read. The polygons come from the venue's own `sectors` once per
    // (venue, surface, projector) and are indexed by the layer's slot, so the
    // `k`-th source of a fan-in surface takes the `k`-th cell. A legacy
    // surface answers exactly one cell (the whole plane), which keeps the
    // whole-surface path byte-identical.
    let mut splits: HashMap<(String, String), Vec<Vec<[f32; 2]>>> = HashMap::new();
    let layer_sectors: Vec<Option<Vec<[f32; 2]>>> = list
        .iter()
        .map(|l| {
            let binding = l.surface.as_ref()?;
            let v = venues.get(&binding.venue)?;
            let cells = splits
                .entry((binding.venue.clone(), binding.surface.clone()))
                .or_insert_with(|| v.sector_splits(device, &binding.surface));
            let i = l.sector_of.unwrap_or(0);
            // More sources than cells wraps onto the cells that exist, so a
            // sixth branch of a five-cell surface lands on the first again
            // rather than drawing nowhere.
            cells
                .get(i % cells.len().max(1))
                .filter(|p| p.len() >= 3)
                .cloned()
        })
        .collect();
    // The cell count, for the same layers, so the panel can draw the grid it
    // is dealing without asking the venue again. Read off the cache the
    // polygons above came from, not recomputed.
    for l in list.iter_mut() {
        if let Some(binding) = &l.surface {
            l.sector_count = splits
                .get(&(binding.venue.clone(), binding.surface.clone()))
                .map(|c| c.len())
                .unwrap_or(1)
                .max(1);
        }
    }

    // A clip resolves its `parent`/`compose` against the device's own masks
    // plus this output's projected wall contours; persisted first, so a 2D
    // mask resolves exactly as it did.
    let mut combined_context: Vec<Mask> = Vec::new();
    let mask_context: &[Mask] = if projected.is_empty() {
        &device_masks
    } else {
        combined_context.extend(device_masks.iter().cloned());
        combined_context.extend(projected.iter().cloned());
        &combined_context
    };

    // A layer's clip: its device contour (R22.6) or, for a wall binding, the
    // named contour projected to this output. A whole-wall binding has no
    // clip — it fills the surface and nothing cuts it.
    let base_clip_of = |l: &Layer| -> Option<Mask> {
        match &l.mask_wall {
            Some(binding) => binding
                .contour
                .as_deref()
                .and_then(|cid| projected.iter().find(|m| m.id == cid))
                .cloned(),
            None => l
                .mask
                .as_deref()
                .and_then(|id| device_masks.iter().find(|m| m.id == id))
                .cloned(),
        }
    };
    let place_of = |l: &Layer,
                   m: Option<&Mask>,
                   warp: Option<&compose::Warp>,
                   mesh: Option<&compose::WarpMesh>,
                   cell: Option<&Vec<[f32; 2]>>|
     -> Sector {
        // P2: a source pinned to a surface renders at the projected
        // contour's bounding box × 1.5, not at the device size — the
        // surface it lands on is usually a patch of the frame, and a full
        // frame rendered to throw most of it away is most of a pass.
        if let Some(binding) = &l.surface {
            // P-V: a cell of the surface's grid places the layer at the
            // cell's own bounding box, and the cell's polygon (a clip above)
            // decides which pixels in it land. One cell — a legacy surface —
            // is the whole plane, so this is the projected bbox as before.
            if let Some(poly) = cell {
                return sector_place(poly);
            }
            // P3: a mesh's own bbox is the place; a planar surface still uses
            // the homography's exact projected bbox.
            if let Some(mesh) = mesh {
                return mesh_place(mesh);
            }
            if let Some(warp) = warp {
                return surface_place(&warp.homography);
            }
            // P2.4: a surface the venue can read but cannot map (no pins,
            // no fit) has no place on the frame — never the whole wall.
            // A surface whose venue cannot be read at all falls through to
            // the flat path (`no_surface_node_means_no_warp`).
            if venues.contains_key(&binding.venue) {
                return Sector { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
            }
        }
        match m {
            Some(m) if m.closed() => m.bounds_in(mask_context),
            // A wall binding with no named contour (or a contour the scene
            // cannot see) fills the output, so the whole frame is where to
            // look; the warp decides which pixels land. The layer's own
            // `sector`/`giro` are the grid's vocabulary, not the wall's.
            _ if l.mask_wall.is_some() => Sector { x: 0.0, y: 0.0, w: 1.0, h: 1.0 },
            _ => l.sector,
        }
    };
    // P-V: a source dealt to one cell of its surface's grid is cut to that
    // cell, whatever contour the mask chain also names: the cell is the
    // output's own division of the wall, the nearer, later statement of where
    // the picture goes.
    let clip_of = |l: &Layer, cell: Option<&Vec<[f32; 2]>>| -> Option<Mask> {
        if let Some(poly) = cell {
            return Some(Mask {
                id: format!("{}#sector", l.id),
                name: l.name.clone(),
                points: poly.clone(),
                ..Default::default()
            });
        }
        base_clip_of(l)
    };
    let clips: Vec<Option<Mask>> = list
        .iter()
        .zip(layer_sectors.iter())
        .map(|(l, cell)| clip_of(l, cell.as_ref()))
        .collect();
    let places: Vec<Sector> = list
        .iter()
        .zip(clips.iter())
        .zip(layer_warps.iter())
        .zip(layer_sectors.iter())
        .map(|(((l, m), (warp, mesh)), cell)| {
            place_of(l, m.as_ref(), warp.as_ref(), mesh.as_ref(), cell.as_ref())
        })
        .collect();

    // The budget is charged once per unique `gen.*` node, not once per layer
    // that happens to land on it — `dedup_key[i]` is which node
    // `node_of_layer[i]` is, by first-seen order, so two layers sharing a
    // generator share a key.
    let mut node_seen: Vec<&str> = Vec::with_capacity(node_of_layer.len());
    let dedup_key: Vec<Option<usize>> = node_of_layer
        .iter()
        .map(|id| {
            let idx = match node_seen.iter().position(|s| *s == id.as_str()) {
                Some(i) => i,
                None => {
                    node_seen.push(id.as_str());
                    node_seen.len() - 1
                }
            };
            Some(idx)
        })
        .collect();
    let (_sizes, _scale) = compose::plan_dedup(w, h, &limits, &places, &dedup_key);

    // P2: the render size each generator pinned to a surface draws at — the
    // projected bounding box × 1.5, in pixels. P3.3: a generator whose layer
    // spans devices draws at the covering canvas size instead of at this
    // device's size, so every device's window crops out of one real canvas
    // rather than being upscaled out of a device-sized one. Every other
    // generator still draws at the device size and the frame is unchanged.
    //
    // TODO(P3.3): the canvas is still rendered once per device — `render_device`
    // is the per-device pass. Sharing one canvas across the devices a layer
    // spans is what would make "once" literal; the render and the crop here
    // are already canvas-sized and per-window.
    let gen_size: HashMap<metal_graph::NodeId, (u32, u32)> = list
        .iter()
        .zip(node_of_layer.iter())
        .zip(layer_warps.iter())
        .zip(layer_sectors.iter())
        .filter_map(|(((l, id), (warp, mesh)), cell)| {
            if l.surface.is_some() {
                // P-V: a source in one cell renders at that cell's bbox × 1.5,
                // not at the whole surface's.
                if let Some(poly) = cell {
                    return Some((id.clone(), render_size_from_bbox(bbox_of(poly), w, h)));
                }
                // P3: a mesh-warped layer still gets a render size, from its
                // mesh bbox; a planar one keeps the homography path.
                if let Some(mesh) = mesh.as_ref() {
                    return Some((id.clone(), mesh_render_size(mesh, w, h)));
                }
                let warp = warp.as_ref()?;
                return Some((id.clone(), surface_render_size(&warp.homography, w, h)));
            }
            (!l.span.is_empty()).then(|| (id.clone(), span_canvas_size(&l.span, w, h)))
        })
        .collect();

    // K: `fx.speed` — advance each generator's own clock by `dt × speed` (the
    // product of every `fx.speed` in its chain) and draw it at that time. The
    // bindings above have run, so a bound `fx.speed` amount is already in
    // `graph`. A graph with no `fx.speed` at all resolves every clock to the
    // show time, so this is the identity for every existing setup.
    let gen_time = state.clocks.advance(device, &graph, time);
    stamp_beats(&mut graph, sim, time);
    let mut result = paint.run_full(
        &graph, &plan, &rows, &fondo_alpha, &frame.raw, time, w, h, &gen_time, &gen_size, true,
    );

    // Which plan buffer each layer's generator landed in, so the frame `run`
    // handed back can be matched back to the layer that asked for it. A
    // shared generator's two layers resolve to the same buffer here, which
    // is the point — one readback, two tiles.
    let buffer_of: std::collections::HashMap<&str, usize> = plan
        .ops
        .iter()
        .filter_map(|op| op.buffer.map(|b| (op.node.as_str(), b)))
        .collect();

    // Which node's buffer actually holds a layer's finished picture.
    //
    // With no `fx.*` nodes — every synthesized graph — that is the generator
    // itself, and this is the identity. With an authored chain it is the
    // *tail*: `Paint::run` forwards each effect node's predecessor buffer
    // along the chain, and only the tail's buffer is a sink input, so looking
    // the generator up here would find a buffer that `run` filtered out and
    // drop the layer from the composition entirely. Silent, in range, and a
    // blank screen — the exact failure class §3.3 names.
    let sink_of = |gen_id: &str| -> String {
        metal_graph::effects::chain(&graph, gen_id)
            .last()
            .cloned()
            .unwrap_or_else(|| gen_id.to_string())
    };

    // `Layer::span`: a source spanning several devices shows only its window
    // here. `gen_size` rendered it once at the covering canvas size, so this
    // crops a real canvas — the window is `0..1` of the picture, the same
    // space the canvas holds, and `crop_window` clamps a window that reaches
    // past the canvas's edge. The crop becomes a tile the compositor scales
    // into the sector the way it scales a budget-shrunk one. `None` — every
    // layer with no window for this device — borrows the frame as before.
    // G2: a spanning layer's texture is cropped on the GPU (a rectangle copy);
    // only a painter without that (or a CPU frame) takes the bytes path below.
    let mut tex_crops: Vec<Option<crate::gpu_frame::GpuFrame>> = vec![None; list.len()];
    if !result.textures.is_empty() {
        for (i, (layer, gen_id)) in list.iter().zip(node_of_layer.iter()).enumerate() {
            let Some(win) = layer.span.get(device) else { continue };
            if let Some(&b) = buffer_of.get(sink_of(gen_id).as_str()) {
                if let Some(t) = result.textures.get(&b) {
                    match paint.crop_texture(t, win) {
                        Some(c) => tex_crops[i] = Some(c),
                        None if crop_rect(t.width(), t.height(), win).is_none() => {}
                        None => {
                            let t = result.textures.remove(&b).expect("just seen");
                            let px = paint.readback(&t);
                            result.frames.insert(b, (px, t.width(), t.height()));
                        }
                    }
                }
            }
        }
    }
    let crops: Vec<Option<(Vec<u8>, u32, u32)>> = list
        .iter()
        .zip(node_of_layer.iter())
        .map(|(layer, gen_id)| {
            let win = layer.span.get(device)?;
            let buf = buffer_of.get(sink_of(gen_id).as_str())?;
            let (px, pw, ph) = result.frames.get(buf)?;
            crop_window(px, *pw, *ph, win)
        })
        .collect();

    // Built in lockstep with `warps`, so `warps[i]` always belongs to
    // `tiles[i]` even though a layer whose buffer is missing is skipped.
    // Remote layers: each is asked for its picture with the state this
    // machine resolved (model, row, chain, time) and placed, warped and cut
    // like a local tile. The tile goes in just before the first local tile at
    // or above its `pos`.
    let graph_frame = digests.graph_frame(&graph);
    // (index into `remote_layers`, RGBA, w, h, where it lands, its contour,
    // its warp and its mesh)
    let mut remote_px: Vec<(usize, Vec<u8>, u32, u32, Sector, Option<Mask>, Option<compose::Warp>, Option<compose::WarpMesh>)> = Vec::new();
    for (ri, r) in remote_layers.iter().enumerate() {
        let layer = &r.layer;
        let model = layer.showing(&air, models);
        let base: Vec<f32> = layer
            .base
            .clone()
            .unwrap_or_else(|| state.base[model.min(state.base.len() - 1)].clone());
        // The digest a local generator of this node would read: the one wired
        // to it (`digests.wired`), or the graph's own frame for a synthesized
        // graph, which has no per-node wiring.
        // A graph-named generator's row is the one the pass above drove, with
        // its bindings and script outputs already landed. A synthesized graph
        // has no node to bind, so its row is built here as it always was.
        let mut row = match &r.gen {
            Some(gen) => rows.get(gen).cloned().unwrap_or_default(),
            None => {
                let wired = Some(&graph_frame);
                let mut row = row_for_node(None, None, paint, state, model, &base, wired, time, passage, layer.library.as_deref());
                if manifest_ids.get(model).copied() == Some(crate::forge::MODEL) {
                    crate::forge::ingest_row(&mut row, wired.unwrap_or(&Digests::zero()));
                }
                apply_node_post(&mut row, &layer.effects, layer.fijo_mask);
                row
            }
        };
        // The same `fx.*` chain a local generator gets, resolved here from
        // this device's graph and sent with the state.
        let chain = match &r.gen {
            Some(gen) => crate::chain_for_row(&graph, gen, &mut row, &graph_frame.raw),
            None => Vec::new(),
        };
        // A remote layer is dealt the same cell a local one would be, from
        // this device's own reading of the venue.
        let cell = layer.surface.as_ref().and_then(|binding| {
            let v = venues.get(&binding.venue)?;
            let cells = splits
                .entry((binding.venue.clone(), binding.surface.clone()))
                .or_insert_with(|| v.sector_splits(device, &binding.surface));
            let i = layer.sector_of.unwrap_or(0);
            cells.get(i % cells.len().max(1)).filter(|p| p.len() >= 3).cloned()
        });
        let clip = clip_of(layer, cell.as_ref());
        let (warp, mesh) = warp_of(layer);
        let place = place_of(layer, clip.as_ref(), warp.as_ref(), mesh.as_ref(), cell.as_ref());
        // A surface-pinned layer is drawn at the size the local path renders
        // it at (`surface_render_size`), so the warp samples the same pixels.
        // The same render size the local generator draws at (`gen_size`): a
        // surface-pinned layer at its projected size, a spanning one at the
        // covering canvas, anything else at the device size.
        let span_size = (!layer.span.is_empty()).then(|| span_canvas_size(&layer.span, w, h));
        let (aw, ah) = match &layer.surface {
            Some(_) => match warp_bbox(warp.as_ref(), mesh.as_ref()) {
                Some(bbox) => render_size_from_bbox(Some(bbox), w, h),
                None => (w, h),
            },
            None => span_size.unwrap_or((w, h)),
        };
        // The clock the local generator draws at: its `fx.speed` clock.
        let node_time = r.gen.as_ref().and_then(|g| gen_time.get(g)).copied().unwrap_or(time);
        let ask = RemoteAsk {
            layer,
            node: &r.node,
            model,
            row: &row,
            chain: &chain,
            time: node_time,
            fondo_alpha: layer.fondo_alpha,
            speed: r.gen.as_deref().map_or(1.0, |g| crate::motion::speed_of(&graph, g)),
            frame: (time * 60.0) as u64,
            w: aw,
            h: ah,
        };
        // Unavailable (no peer, not ready, wrong size): left out, never waited for.
        let Some((pw, ph, rgb)) = remote(&ask) else { continue };
        if pw != aw || ph != ah {
            continue;
        }
        let rgba: Vec<u8> = rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        // A spanning layer's picture is the covering canvas: this device's
        // window of it, as the local crop takes it. No window here: the whole
        // canvas, as the local tile does.
        let (rgba, pw, ph) = match layer.span.get(device).and_then(|win| crop_window(&rgba, pw, ph, win)) {
            Some(cropped) => cropped,
            None => (rgba, pw, ph),
        };
        remote_px.push((ri, rgba, pw, ph, place, clip, warp, mesh));
    }

    // Parity P-E: a source inside its in/out transition gets the transition
    // pass on its finished picture — a pure read of the clock against the
    // toggle instant (`transition`), owned here so the tiles can borrow it.
    let lifted = lifted.into_inner();
    // G2: a source inside its transition. A texture tile goes through the
    // transition pass and stays a texture; a CPU tile (a spanning crop, a
    // Cpu backend) takes the bytes path.
    enum Faded {
        Cpu(Vec<u8>, u32, u32),
        Tex(crate::gpu_frame::GpuFrame),
    }
    let faded: Vec<Option<Faded>> = if state.transitions.is_empty() {
        Vec::new()
    } else {
        list.iter()
            .zip(node_of_layer.iter())
            .zip(crops.iter().zip(tex_crops.iter()))
            .map(|((_, gen_id), (crop, tex_crop))| {
                let phase = state.transitions.frame_phase(
                    &graph, gen_id, &lifted, time, bpm, &state.transition_presets,
                )?;
                let (v, seed) = (phase.visibility(), (time * 60.0) as u32);
                if let Some(t) = tex_crop {
                    if v >= 1.0 {
                        return None;
                    }
                    if let Some(out) = paint.transition_texture(&phase.spec, v, t, seed) {
                        return Some(Faded::Tex(out));
                    }
                    let mut px = paint.readback(t);
                    paint.transition(&phase.spec, v, &mut px, t.width(), t.height(), seed);
                    return Some(Faded::Cpu(px, t.width(), t.height()));
                }
                let (mut px, pw, ph) = match crop {
                    Some((px, pw, ph)) => (px.clone(), *pw, *ph),
                    None => {
                        let buf = buffer_of.get(sink_of(gen_id).as_str())?;
                        if let Some(t) = result.textures.get(buf) {
                            if v >= 1.0 {
                                return None;
                            }
                            if let Some(out) = paint.transition_texture(&phase.spec, v, t, seed) {
                                return Some(Faded::Tex(out));
                            }
                            (paint.readback(t), t.width(), t.height())
                        } else {
                            let (px, pw, ph) = result.frames.get(buf)?;
                            (px.clone(), *pw, *ph)
                        }
                    }
                };
                paint.transition(&phase.spec, v, &mut px, pw, ph, seed);
                Some(Faded::Cpu(px, pw, ph))
            })
            .collect()
    };
    // One entry per layer that has a picture, back to front: where its
    // pixels are (a texture or a CPU buffer) and everything compose needs.
    let mut entries: Vec<Entry> = Vec::with_capacity(list.len());
    // The `list` index each entry came from: where a remote entry goes is
    // decided against these.
    let mut tile_pos: Vec<usize> = Vec::with_capacity(list.len());
    for (i, (((layer, gen_id), (place, clip)), (crop, (warp, mesh)))) in list
        .iter()
        .zip(node_of_layer.iter())
        .zip(places.iter().zip(clips.iter()))
        .zip(crops.iter().zip(layer_warps.iter()))
        .enumerate()
    {
        let tex_crop = tex_crops[i].as_ref();
        // P2.4: a layer pinned to a surface the venue can read but cannot map
        // (no pins, no fit) draws nothing — no whole-wall fallback. A surface
        // node whose venue cannot be read at all keeps the flat path
        // (`no_surface_node_means_no_warp`).
        if let Some(binding) = &layer.surface {
            let mappable = warp_bbox(warp.as_ref(), mesh.as_ref()).is_some();
            if venues.contains_key(&binding.venue) && !mappable {
                continue;
            }
        }
        let fade = faded.get(i).and_then(Option::as_ref);
        let px = match (fade, crop.as_ref()) {
            (Some(Faded::Tex(t)), _) => Px::Tex(t),
            (None, None) if tex_crop.is_some() => Px::Tex(tex_crop.expect("checked")),
            (Some(Faded::Cpu(px, pw, ph)), _) | (None, Some((px, pw, ph))) => Px::Cpu(px, *pw, *ph),
            (None, None) => {
                let Some(buf) = buffer_of.get(sink_of(gen_id).as_str()) else { continue };
                if let Some(t) = result.textures.get(buf) {
                    Px::Tex(t)
                } else {
                    let Some((px, pw, ph)) = result.frames.get(buf) else { continue };
                    Px::Cpu(px, *pw, *ph)
                }
            }
        };
        entries.push(Entry {
            px,
            layer,
            place: *place,
            clip: clip.as_ref(),
            warp: *warp,
            mesh: mesh.clone(),
        });
        tile_pos.push(i);
    }
    // Each remote entry lands below the first local entry at or above its `pos`.
    for (ri, rgba, pw, ph, place, clip, warp, _mesh) in &remote_px {
        let pos = remote_layers[*ri].pos;
        let at = tile_pos.iter().position(|&p| p >= pos).unwrap_or(entries.len());
        entries.insert(
            at,
            Entry {
                px: Px::Cpu(rgba, *pw, *ph),
                layer: &remote_layers[*ri].layer,
                place: *place,
                clip: clip.as_ref(),
                warp: *warp,
                // Remote mesh warps are not carried yet: the remote picture
                // is composited flat through any homography it has.
                mesh: None,
            },
        );
        tile_pos.insert(at, pos);
    }

    // G1/G3: the GPU backend composes the stack in one pass. A contour clip is
    // a device-sized coverage map (the polygon walked once on the CPU, cached
    // while the masks stand still) and a warp is its inverse map; both are read
    // by the compose shader, so neither reads a texture back.
    //
    // P3: the mesh path is CPU-only for now. A mesh warp cannot go through
    // `compose::warp_matrix` (a homography), so when any entry carries one the
    // whole frame takes the CPU composite below. Frames with only planar /
    // homography warps still compose on the GPU exactly as before.
    let has_mesh = entries.iter().any(|e| e.mesh.is_some());
    let gpu_frame = if !has_mesh && paint.backend() == crate::backend::Backend::Gpu {
        let mut covers: Vec<Option<crate::gpu_frame::CoverRef>> = Vec::with_capacity(entries.len());
        let mut covered = true;
        for e in &entries {
            let Some(clip) = e.clip.filter(|m| m.closed()) else {
                covers.push(None);
                continue;
            };
            let (_, y0, _, sh) = e.place.pixels(w, h);
            let key = output::clip_cover_key(mask_context, clip, w, h, y0, y0 + sh);
            match paint.cover_gpu(key, w, h, &mut || output::clip_cover_bytes(mask_context, clip, w, h, y0, y0 + sh)) {
                Some(c) => covers.push(Some(c)),
                None => {
                    covered = false;
                    break;
                }
            }
        }
        if covered {
            let tiles: Vec<crate::gpu_frame::GpuTile> = entries
                .iter()
                .zip(&covers)
                .map(|(e, cover)| {
                    let (tw, th) = match &e.px {
                        Px::Tex(t) => (t.width(), t.height()),
                        Px::Cpu(_, pw, ph) => (*pw, *ph),
                    };
                    crate::gpu_frame::GpuTile {
                        source: match &e.px {
                            Px::Tex(t) => crate::gpu_frame::TileSource::Texture(t),
                            Px::Cpu(p, pw, ph) => crate::gpu_frame::TileSource::Cpu { pixels: p, w: *pw, h: *ph },
                        },
                        sector: e.place,
                        opacity: e.layer.opacity.clamp(0.0, 1.0),
                        blend: e.layer.blend,
                        turn: e.layer.turn,
                        giro_mode: e.layer.giro_mode,
                        cover: cover.as_ref(),
                        warp: e.warp.and_then(|wp| {
                            compose::warp_matrix(&wp, tw, th, w, h).map(|m| crate::gpu_frame::GpuWarp {
                                m,
                                nearest: wp.sampling == compose::Sampling::Nearest,
                            })
                        }),
                    }
                })
                .collect();
            paint.compose_gpu(w, h, composition, &tiles)
        } else {
            None
        }
    } else {
        None
    };
    // Texture entries the CPU compose needs as bytes.
    let owned: Vec<Option<Vec<u8>>> = if gpu_frame.is_some() {
        Vec::new()
    } else {
        entries
            .iter()
            .map(|e| match &e.px {
                Px::Tex(t) => Some(paint.readback(t)),
                Px::Cpu(..) => None,
            })
            .collect()
    };
    let mut frame_out: Vec<u8> = Vec::new();
    if gpu_frame.is_none() {
        let mut tiles: Vec<compose::Tile> = Vec::with_capacity(entries.len());
        let mut warps: Vec<Option<compose::Warp>> = Vec::with_capacity(entries.len());
        let mut meshes: Vec<Option<compose::WarpMesh>> = Vec::with_capacity(entries.len());
        for (e, bytes) in entries.iter().zip(&owned) {
            let (px, pw, ph): (&[u8], u32, u32) = match (&e.px, bytes) {
                (Px::Cpu(p, pw, ph), _) => (p, *pw, *ph),
                (Px::Tex(t), Some(b)) => (b, t.width(), t.height()),
                (Px::Tex(_), None) => continue,
            };
            // `Warp` is `Copy`; a `None` here is the un-warped tile, byte for
            // byte the path every tile took before Task #8.
            tiles.push(compose::Tile::of(e.layer, px, pw, ph, e.place, e.clip, mask_context));
            warps.push(e.warp);
            meshes.push(e.mesh.clone());
        }
        frame_out = if meshes.iter().any(|m| m.is_some()) {
            compose::compose_rgba_warped_mesh(
                w, h, composition, &tiles, &warps, &meshes, Some([0, 0, 0, 255]),
            )
        } else if warps.iter().any(|w| w.is_some()) {
            compose::compose_warped(w, h, composition, &tiles, &warps)
        } else {
            compose::compose(w, h, composition, &tiles)
        };
    }
    drop(entries);

    // What follows compose, in the CPU path's own order: the overlay cover,
    // edge blend, the mask strokes, the 3D cut. While the frame is a texture
    // each is a shade pass; the first one that cannot run (a CPU backend, a
    // blend map of another size) reads the frame back once and this and
    // every later stage take the CPU code below.
    let blend_maps: Vec<std::sync::Arc<crate::edge_blend::BlendMap>> =
        venues.values().filter_map(|v| cached_blend_map(v, device, w, h)).collect();
    let graph_cuts = graph
        .nodes
        .iter()
        .any(|n| n.kind == "mask" && !n.bypass && mask_node_id(n).is_some());
    let declares_3d_cut = graph.nodes.iter().any(|n| {
        n.kind == "mask"
            && !n.bypass
            && matches!(mask_node_binding(n), Some(MaskBinding::Wall(w)) if w.contour.is_some())
    });
    let _sampled = graph.nodes.iter().any(|n| !n.bypass && crate::wire::frame_driven(n));
    // The wired stroke materials, by mask id (the tiles `Paint::run` kept).
    let stroke_materials: std::collections::HashMap<String, output::StrokeMaterial> = if graph_cuts {
        graph
            .nodes
            .iter()
            .filter(|n| n.kind == "mask" && !n.bypass)
            .filter_map(|n| {
                let mask_id = mask_node_id(n)?;
                let tile = result.materials.get(&(n.id.clone(), "stroke".to_string()))?;
                Some((mask_id, output::StrokeMaterial { pixels: &tile.0, width: tile.1, height: tile.2 }))
            })
            .collect()
    } else {
        Default::default()
    };
    // Task #8: the wall contours the layers were cut to, for the device-wide
    // half of the 3D cut.
    let mut transient: Vec<Mask> = Vec::new();
    for (layer, clip) in list.iter().zip(clips.iter()) {
        if layer.mask_wall.is_none() {
            continue;
        }
        if let Some(clip) = clip {
            if !transient.iter().any(|m| m.id == clip.id) {
                transient.push(clip.clone());
            }
        }
    }
    let (mut overlay_done, mut blend_done, mut strokes_done, mut cut_done) = (false, false, false, false);
    let mut blend_applied = 0usize;
    let mut gpu_frame = gpu_frame;
    if let Some(mut f) = gpu_frame.take() {
        let mut on = true;
        if on {
            if state.overlays.cover(device, time).is_some() {
                match state.overlays.paint_gpu(paint, &f, device, time, &device_masks) {
                    Some(n) => {
                        f = n;
                        overlay_done = true;
                    }
                    None => on = false,
                }
            } else {
                overlay_done = true;
            }
        }
        if on {
            for map in &blend_maps {
                match paint.edge_blend_gpu(&f, map) {
                    Some(n) => {
                        f = n;
                        blend_applied += 1;
                    }
                    None => {
                        on = false;
                        break;
                    }
                }
            }
            blend_done = on;
        }
        if on {
            if graph_cuts {
                match output::apply_masks_gpu(paint, &f, &device_masks, &[], composition, &stroke_materials, false) {
                    Some(n) => {
                        f = n;
                        strokes_done = true;
                    }
                    None => on = false,
                }
            } else {
                strokes_done = true;
            }
        }
        if on {
            if declares_3d_cut {
                match output::apply_masks_gpu(paint, &f, &device_masks, &transient, composition, &Default::default(), true) {
                    Some(n) => {
                        f = n;
                        cut_done = true;
                    }
                    None => {}
                }
            } else {
                cut_done = true;
            }
        }
        if !(overlay_done && blend_done && strokes_done && cut_done) {
            frame_out = paint.readback(&f);
        }
        gpu_frame = Some(f);
    }
    let on_gpu = gpu_frame.is_some() && overlay_done && blend_done && strokes_done && cut_done;
    // `S.O.2`: whatever colour covers this device right now — a timed
    // blackout, a solid for N seconds — goes on last, over the finished
    // composite and before `apply_masks` cuts it to the contours; read live
    // off `state.overlays` at this instant (regla 4). See `overlay.rs`'s
    // "from state to pixels" section. The device's masks travel along for a
    // solid's per-mask test colours (a venue's `rgb` per mask).
    if !overlay_done {
        state.overlays.paint_masked(&mut frame_out, w, h, device, time, &device_masks);
    }

    // P-F: edge blending. Each venue this frame reads (the ones its
    // `venue.surface` layers bind) whose projectors share a surface with
    // `blend.enabled` multiplies this device's frame by its blend map, after
    // the overlays so a test pattern shows the blend. No enabled blend → no
    // map → the frame is untouched, byte for byte.
    if !blend_done {
        for map in blend_maps.iter().skip(blend_applied) {
            map.apply(&mut frame_out);
        }
    }

    // L22.12b / addendum D7: the mask strokes, on the path that never reaches
    // `apply_masks`. A device whose graph carries a live `mask` node is cut
    // per layer (R22.6 decision 3), so the whole-frame pass — and the
    // `apply_mask_strokes` inside it — is skipped by `/output.jpg`. The
    // `stroke` port lives on that very node, so this is also the only path a
    // **material** can take: the tiles `Paint::run` kept for the wired chains
    // (`RunOutput::materials`) are looked up by the mask node's id and handed
    // to the same band painter the legacy path uses. No `mask` node: nothing
    // here, and `/output.jpg`'s whole-frame pass stands exactly as before.
    if graph_cuts && !strokes_done {
        let materials: std::collections::HashMap<String, output::StrokeMaterial> = graph
            .nodes
            .iter()
            .filter(|n| n.kind == "mask" && !n.bypass)
            .filter_map(|n| {
                let mask_id = mask_node_id(n)?;
                let tile = result.materials.get(&(n.id.clone(), "stroke".to_string()))?;
                Some((
                    mask_id,
                    output::StrokeMaterial {
                        pixels: &tile.0,
                        width: tile.1,
                        height: tile.2,
                    },
                ))
            })
            .collect();
        output::apply_mask_strokes(&mut frame_out, w, h, &device_masks, &materials);
    }

    // Task #8: the 3D cut. A graph that declares a wall contour cuts the
    // finished frame here, with persisted ∪ projected contours — the pass the
    // whole-frame caller runs, which now skips a device the graph itself cuts
    // (`graph_has_mask_node`). The per-layer clip above already shaped each
    // warped source; this is the device-wide half, so a source *not* bound to
    // the wall still ends where the contour does. Gated on a 3D cut being
    // declared, so a 2D-only graph and the no-escena path are untouched.
    if declares_3d_cut && !cut_done {
        output::apply_masks_with(&mut frame_out, w, h, &device_masks, &transient, composition);
    }

    // S.AL.1: the fixture half of this frame — `drive_head` already computes
    // the DMX slots a head would be sent (`crate::fixture`'s own module
    // doc), and this is the first real caller that puts them on a wire.
    // Only a device with a head does anything here; a screen's graph may
    // still carry an `out.sacn` node (unusual, harmless) but `head` being
    // `None` means there is nothing to drive it with, so the whole walk is
    // skipped rather than sending zeros nobody asked for.
    if let Some(head) = &head {
        let cover = state.overlays.cover(device, time);
        state.wire_errors += crate::wire::drive_frame(
            &graph,
            head,
            &output::data_root(),
            cover,
            &rows,
            &script_rows,
            &frame.raw,
            &mut state.wire_sockets,
            &mut state.bluetooth_links,
            &(metal_ble::open as fn(metal_ble::LinkConfig) -> Box<dyn metal_ble::Link>),
            web_time::Instant::now(),
        );
    }

    // Frame-driven fixtures (a venue's `drive = "frame"` washes, spots and
    // LED strips): the `out.*` nodes of this device's own graph that sample
    // the frame just composed, overlays included — `crate::wire::drive_device`.
    // A texture frame is read through the sink's own non-blocking ring (a
    // frame or two old, which a fixture wire cannot tell), and only when the
    // device's graph has a node that samples it; `None` while the first copy is
    // in flight, so that frame drives nothing.
    // Fixtures F3/F4: the rows a `venue.fixture` node reads when its colour is
    // not the frame — the model rows and their base rows, the script rows, and
    // the digest frame the sources were driven from.
    let source_rows = crate::wire::SourceRows {
        rows: Some(&rows),
        base: Some(&base_rows),
        script_rows: Some(&script_rows),
        raw: &frame.raw,
    };
    match (&gpu_frame, on_gpu) {
        (Some(f), true) => {
            let key = format!("wires:{device}");
            state.wire_errors += crate::wire::drive_device_with(
                &output::data_root(),
                device,
                Some(&graph),
                &source_rows,
                || paint.readback_async(&key, f).map(|(b, w, h)| (std::borrow::Cow::Owned(b), w, h)),
                &mut state.sampled_wires,
                &mut state.dmx_bus,
                web_time::Instant::now(),
            );
        }
        _ => {
            state.wire_errors += crate::wire::drive_device(
                &output::data_root(),
                device,
                Some(&graph),
                &source_rows,
                &frame_out,
                w,
                h,
                &mut state.sampled_wires,
                &mut state.dmx_bus,
                web_time::Instant::now(),
            );
        }
    }

    match gpu_frame {
        Some(f) if on_gpu => DeviceFrame::Gpu(f),
        _ => DeviceFrame::Cpu(frame_out, w, h),
    }
}

/// G3: a clamped-size device frame stretched (nearest) to the size asked for.
fn upscale_device_frame(paint: &mut dyn Paint, f: DeviceFrame, w: u32, h: u32) -> DeviceFrame {
    match f {
        DeviceFrame::Gpu(g) => match paint.upscale_gpu(&g, w, h) {
            Some(up) => DeviceFrame::Gpu(up),
            None => DeviceFrame::Cpu(upscale_rgba(&paint.readback(&g), g.width(), g.height(), w, h), w, h),
        },
        DeviceFrame::Cpu(px, sw, sh) => DeviceFrame::Cpu(upscale_rgba(&px, sw, sh, w, h), w, h),
    }
}

fn upscale_rgba(px: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Vec<u8> {
    let (sw, sh) = (sw.max(1) as usize, sh.max(1) as usize);
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h as usize {
        let sy = (y * sh / h.max(1) as usize).min(sh - 1);
        for x in 0..w as usize {
            let sx = (x * sw / w.max(1) as usize).min(sw - 1);
            if let Some(src) = px.get((sy * sw + sx) * 4..(sy * sw + sx) * 4 + 4) {
                out[(y * w as usize + x) * 4..][..4].copy_from_slice(src);
            }
        }
    }
    out
}

/// O.6's missing route: a script's row, arriving on a `mask` node's
/// `control` port, **is the contour** for this frame.
///
/// For every `mask` node in `graph` whose `control` in-edge comes from a
/// node `script_rows` has a row for, and whose config names a device mask
/// (`{"mask": "<Output.masks id>"}` — the same key a `gen.*` member node
/// binds a layer with, `layer_node_config`), the row is read as `x, y, x,
/// y, …` in device space (0..1) and written over that mask's `points` in
/// `masks` — the per-frame copy `render_device` holds, never the saved
/// list. Every layer bound to that mask id then clips to the script's
/// shape through the same `clips`/`bounds_in`/`Tile::of` path a hand-drawn
/// contour takes; there is no second mask type and no second compositing
/// path (regla 6).
///
/// The row is the polygon rather than some `(side, width)` vocabulary this
/// function would have to know: a script *transforms* — it decides where the
/// contour goes and writes it — and a mask node should not have to agree
/// with every script about what two numbers mean. Fewer than three points
/// (under six floats, or an odd count's trailing float) leaves the mask
/// unchanged, the same "not a shape yet" reading `Mask::closed` gives an
/// unfinished contour, so a script that answered nothing useful this frame
/// does not black out the layers on it. A `compose`d mask ignores `points`
/// by definition and is left alone too.
pub fn apply_script_masks(
    graph: &metal_graph::Graph,
    script_rows: &std::collections::HashMap<metal_graph::NodeId, Vec<f32>>,
    masks: &mut [Mask],
) {
    for node in graph.nodes.iter().filter(|n| n.kind == "mask" && !n.bypass) {
        let Some(edge) = graph
            .edges
            .iter()
            .find(|e| e.to.node == node.id && e.to.port == "control")
        else {
            continue;
        };
        let Some(row) = script_rows.get(&edge.from.node) else { continue };
        if row.len() < 6 {
            continue;
        }
        let config: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null);
        let Some(mask_id) = config.get("mask").and_then(Value::as_str) else { continue };
        let Some(mask) = masks.iter_mut().find(|m| m.id == mask_id) else { continue };
        if mask.compose.is_some() {
            continue;
        }
        mask.points = row
            .chunks_exact(2)
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();
    }
}

/// The float row model `showing` should be drawn with, and — if it is a
/// transmission — everything else it needs to draw at all.
///
/// # Why this is one function and not two paths at each call site
///
/// A transmission is *selected the same way*, *composited the same way* and
/// *reported the same way* as any other model; the only thing it does
/// differently is that it has to be handed footage first. Putting that in one
/// place beside `drive::row` is what keeps "a transmission is a model" true
/// everywhere else in this file.
///
/// Two things happen here for a transmission and each is deliberate:
///
/// - **The cue is refreshed once**, out of the app's single Curator, so every
///   screen drawing this model this frame is handed the same shot.
/// - **The library and the cue are pushed to the renderer** every frame rather
///   than on a change. `attach` compares and does nothing when it matches, and
///   the alternative is a renderer whose footage depends on which route ran
///   first after a swap.
///
/// # S.AM: a transmission is no longer the special case for *offsets*
///
/// Before this, a model was graded by the Colourist iff `paint.footage()` said
/// it was a transmission. Now every model's row is driven by the round-robin
/// column assignment unless a wired `ai.*` node answers, and a transmission is
/// just a model that also needs its footage attached. The Colourist itself
/// (`metal_reel::grade`) is untouched; what changed is who chooses to call it —
/// [`row_for_node`] reads the `ai.colourist` node wired to the source's
/// `control` port, and the `footage` flag only decides the cue above.
pub fn row_for(
    paint: &mut dyn Paint,
    state: &mut State,
    showing: usize,
    base: &[f32],
    frame: &digest::Frame,
    time: f32,
    passage: Option<[f32; metal_reel::curate::FEATURES]>,
    library: Option<&str>,
) -> Vec<f32> {
    row_for_node(None, None, paint, state, showing, base, Some(frame), time, passage, library)
}

/// [`row_for`] for a source that is a node of a graph: the `ai.*` node wired
/// into `node`'s `control` port (if any) supplies the offsets, evaluated
/// against the schema of the source that consumes it.
///
/// **Per `(model, schema)`.** A Colourist is fitted for one model, so the node
/// is evaluated *here*, in the source that consumes it, not once for the
/// graph. An `ai.*` fanned out to two sources is evaluated twice — once per
/// schema — and this function is the only place that happens.
///
/// **The inference runs through `metal_logic::AuxLogic`.** The fitted rows are
/// handed to a [`metal_logic::ColouristLogic`] and run under D1's budget; the
/// engine gets no second CPU-before-drawing path, and a failure is a value
/// (here, a fall back to the round robin) rather than a panic.
pub fn row_for_node(
    graph: Option<&metal_graph::Graph>,
    node: Option<&str>,
    paint: &mut dyn Paint,
    state: &mut State,
    showing: usize,
    base: &[f32],
    frame: Option<&digest::Frame>,
    time: f32,
    passage: Option<[f32; metal_reel::curate::FEATURES]>,
    library: Option<&str>,
) -> Vec<f32> {
    let schema = paint.schemas()[showing];
    let id = paint.ids()[showing];
    let footage = paint.footage().get(showing).copied().unwrap_or(false);

    // The Curator's footage cue is untouched: a transmission still gets its
    // library and its standing cue pushed to the renderer every frame. A
    // name that will not load leaves nothing attached, and the model draws
    // its slate — which says on its face that there is no footage.
    if footage {
        if let Some(name) = library {
            state.reels.ensure(name);
        }
        let cue = state.reels.cue(library, time, passage);
        if let Some(reel) = paint.reel(showing) {
            reel.attach(state.reels.library(library).cloned());
            reel.cue(cue);
        }
    }

    // The dials are the source's own (`drive` in its config, plan Design 4).
    let dials = Drive::of_node(graph.and_then(|g| g.node(node?)));
    // The source's own `modo` (2026-10-08): `ai` = the fitted colourist,
    // `ai_fx` = `ai` plus the effects the node carries itself, `manual` = the
    // held row at rest. Absent (or a node saved before the key existed) is
    // `Inherit`, which keeps the pre-`modo` behaviour: no frame at rest, else
    // the fitted grade when one is wired and trained, else the round robin.
    let mode = node_modo(graph.and_then(|g| g.node(node?)));
    let (mut row, wired) = match frame {
        // Manual, or nothing audio wired to `control`: static — depth 0, the
        // operator's base row (plan "Agreed with the operator"). No fallback.
        None => (drive::row(schema, base, &Digests::zero(), &dials.at_rest()), None),
        Some(frame) => match mode {
            // `manual`: the operator's held row even with audio live — the
            // base row, no digest drive, no colourist.
            Modo::Manual => {
                (drive::row(schema, base, &Digests::zero(), &dials.at_rest()), None)
            }
            // `ai` / `ai_fx` (or `inherit`): the fitted grade when one is
            // wired and trained, else the round robin. An untrained node is
            // never a broken node, and `trained() == false` is what the node
            // reports. `ai_fx` is `ai` **plus** the effects the node carries
            // itself (plan §4): the source's own row is identical, and those
            // embedded effect floats are driven by [`embedded_effects`].
            Modo::Ai | Modo::AiFx | Modo::Inherit => {
                let row = match ai_offsets(graph, node, state, schema, id, frame, dials.smooth) {
                    Some(offsets) => drive::row_with(schema, base, frame, &dials, Some(&offsets)),
                    None => drive::row(schema, base, frame, &dials),
                };
                (row, Some(frame))
            }
        },
    };
    // V4 (decision 11): the face solver sits between `drive::row` and the
    // loop, so a lane (which reaches here through `base`, from an overlay or a
    // hand-set preset) or a loop (below) that names a rig float wins per
    // control. It writes only for a rig model, a node with audio wired, and a
    // row that carries a voice block; `lookahead` is `None` live — nothing yet
    // hands the engine a fine sequence's future row, which is the seam
    // `crate::face` is built around.
    if let Some(frame) = wired {
        if frame.voice && crate::face::has_rig(schema) {
            let key = node.unwrap_or("_").to_string();
            let beat = crate::face::beat_phase(state.music_pos);
            let solver = state.faces.entry(key).or_insert_with(crate::face::Face::new);
            crate::face::apply(schema, &mut row, base, frame, None, beat, time, solver);
        }
    }
    // Loops (T7): only the floats and colour slots the generator's loop
    // targets are overwritten; the audio-driven row stays for the rest.
    let choice = loop_choice(graph.and_then(|g| g.node(node?)));
    if let Some(l) = resolve_loop(&state.loops, id, &choice) {
        drive::apply_loop(schema, &mut row, l, state.music_pos, &state.loops);
    }
    row
}

/// What a generator node's config says about its loop (key `loop`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopChoice {
    /// No `loop` key (every graph written before loops): the first factory
    /// template for the model, if there is one.
    Default,
    /// `"loop": "none"` (or `null`): pure audio drive, exactly as before loops.
    None,
    /// `"loop": "<id>"`.
    Id(String),
}

/// Reads `loop` off a generator node's config — see [`LoopChoice`].
pub fn loop_choice(node: Option<&metal_graph::Node>) -> LoopChoice {
    let Some(v) = node.and_then(|n| n.config.get("loop")) else {
        return LoopChoice::Default;
    };
    match v.as_str() {
        Some("none") | None => LoopChoice::None,
        Some(id) => LoopChoice::Id(id.to_string()),
    }
}

/// The loop a generator of `model` plays under `choice`. An id that is not
/// in the library, or aimed at another model, plays nothing rather than a
/// guess.
pub fn resolve_loop<'a>(
    lib: &'a crate::pattern::Library,
    model: &str,
    choice: &LoopChoice,
) -> Option<&'a crate::pattern::Loop> {
    use crate::pattern::TargetKind;
    let target = TargetKind::Model(model.to_string());
    match choice {
        LoopChoice::None => None,
        LoopChoice::Id(id) => lib.get(id).filter(|l| l.target_kind == target),
        LoopChoice::Default => lib.default_for(&target),
    }
}

/// The loop an `fx.*` node plays this frame (the loops design notes T8), or `None`.
///
/// Derived, never stored: a bypassed node (Off) or one whose `control` port
/// has an edge (Reactive — wiring is the explicit, advanced choice) plays no
/// loop; otherwise the node's config `loop` resolves exactly like a
/// generator's ([`loop_choice`]): absent → first factory template for the
/// effect, `"none"`/`null` → plain Fixed, `"<id>"` → that loop if it is aimed
/// at this effect. Any effect that declares parameters
/// (`metal_graph::params::effect_params`) can be looped (T11): a one-amount
/// effect through an `Amount` track, a multi-parameter one (`fx.move`,
/// `fx.particles`, `fx.image-pop`, `fx.forge`, `fx.warp`) through `Float`
/// tracks named after its params. The UI derives a "looped" state from this
/// being `Some` on top of `effects::state_of`'s `Fixed`.
pub fn effect_loop<'a>(
    graph: &metal_graph::Graph,
    fx_node: &str,
    lib: &'a crate::pattern::Library,
) -> Option<&'a crate::pattern::Loop> {
    use crate::pattern::TargetKind;
    if !matches!(metal_graph::effects::state_of(graph, fx_node), metal_schema::EffectState::Fixed(_)) {
        return None;
    }
    let node = graph.node(fx_node)?;
    if metal_graph::params::effect_params(&node.kind).is_empty() {
        return None;
    }
    let target = TargetKind::Effect(node.kind.strip_prefix("fx.")?.to_string());
    match loop_choice(Some(node)) {
        LoopChoice::None => None,
        LoopChoice::Id(id) => lib.get(&id).filter(|l| l.target_kind == target),
        LoopChoice::Default => lib.default_for(&target),
    }
}

/// Where a loopable effect's single amount lives: the config key and its
/// declared range. `None` for effects with several (or no) parameters.
fn effect_amount_param(kind: &str) -> Option<(String, f32, f32)> {
    let params = metal_graph::params::effect_params(kind);
    let [p] = params.as_slice() else { return None };
    // A one-param advanced effect (`fx.texture`, `fx.clarity`, `fx.dehaze`)
    // holds its single value under the spec's own name, not `fixed` — see
    // `metal_graph::params::effect_params`'s advanced arm.
    let key = if kind.starts_with("fx.text.") {
        "amount".to_string()
    } else if kind
        .strip_prefix("fx.")
        .and_then(metal_schema::advanced_effect_floats)
        .is_some()
    {
        p.name.clone()
    } else {
        "fixed".to_string()
    };
    Some((key, p.min, p.max))
}

/// Writes every looped `fx.*` node's values at `pos` into this frame's copy
/// of the graph (the same per-frame slot a binding writes, so
/// `resolved_chain`, `fx.speed`, the text pass and the multi-parameter
/// effects' own readers see them as held values) and, for a one-amount
/// effect, into the effect slot of each generator row whose chain holds it.
/// Runs before the bindings, so a bound value still wins.
///
/// T11: an `Amount` track (or a `Float` track naming a one-amount effect's
/// single param) is the amount, as in T8; a `Float { param }` track on a
/// multi-parameter effect writes `param` under its own config key — where
/// `fx.move`/`fx.particles`/`fx.image-pop`/`fx.forge`/`fx.warp` read it —
/// mapped into the param's declared range. Colour params and names the
/// effect does not declare are ignored. Pure in `(graph, lib, pos)`.
pub fn apply_effect_loops(
    graph: &mut metal_graph::Graph,
    rows: &mut crate::bind::Rows,
    lib: &crate::pattern::Library,
    pos: crate::clock::Position,
) {
    use crate::pattern::{TrackTarget, Value};
    let fx: Vec<String> = graph.nodes.iter().filter(|n| n.kind.starts_with("fx.")).map(|n| n.id.clone()).collect();
    for id in fx {
        let Some(l) = effect_loop(graph, &id, lib) else { continue };
        let kind = graph.node(&id).map(|n| n.kind.clone()).unwrap_or_default();
        let params = metal_graph::params::effect_params(&kind);
        let single = effect_amount_param(&kind);
        for (target, value) in l.eval(pos) {
            let Value::Unit(unit) = value else { continue };
            let param = match &target {
                TrackTarget::Amount => None,
                TrackTarget::Float { param } => Some(param.as_str()),
                TrackTarget::Palette { .. } => continue,
            };
            let is_amount = match (param, single.as_ref()) {
                (None, _) => true,
                (Some(p), Some(_)) => params.first().is_some_and(|s| s.name == p),
                (Some(_), None) => false,
            };
            if is_amount {
                let Some((key, min, max)) = single.clone() else { continue };
                let value = min + unit.clamp(0.0, 1.0) * (max - min);
                crate::bind::set_config_number(graph, &id, &key, value);
                let Some(index) = kind.strip_prefix("fx.").and_then(metal_schema::effect_index) else { continue };
                for gen in metal_graph::effects::generators_of(graph, &id) {
                    if let Some(row) = rows.get_mut(&gen) {
                        if row.len() >= metal_schema::EFFECTS_WIDTH {
                            let at = row.len() - metal_schema::EFFECTS_WIDTH + index;
                            row[at] = value;
                        }
                    }
                }
            } else if let Some(p) = params
                .iter()
                .find(|p| Some(p.name.as_str()) == param && p.kind == metal_graph::params::ParamKind::Float)
            {
                let value = p.min + unit.clamp(0.0, 1.0) * (p.max - p.min);
                crate::bind::set_config_number(graph, &id, &p.name, value);
            }
        }
    }
}

/// Factory templates from `<data>/loops/templates`, the user's from
/// `<data>/loops/user`. Test builds start empty, so no existing row test
/// changes with the shipped templates; loop tests load their own tempdir
/// library (rule 8).
pub fn loops_library() -> crate::pattern::Library {
    if cfg!(test) {
        let none = std::path::Path::new("/nonexistent/fw-loops");
        return crate::pattern::Library::load(none, none);
    }
    let root = output::data_root().join("loops");
    crate::pattern::Library::load(&root.join("templates"), &root.join("user"))
}

/// The signed offsets an `ai.*` node wired into `node`'s `control` port wants
/// this frame, or `None` when there is no such node or its model is untrained.
///
/// The fit is read off the graph (regla 4: config is a reading of the graph,
/// never a second state) and handed to [`metal_logic::ColouristLogic`] — the
/// D1 runtime — under the default budget. A digest row of the wrong width, an
/// over-budget call or an unreadable weights file all answer `None`, which the
/// caller turns into the round-robin fallback.
fn ai_offsets(
    graph: Option<&metal_graph::Graph>,
    node: Option<&str>,
    state: &mut State,
    schema: &metal_schema::Schema,
    model: &str,
    frame: &digest::Frame,
    smooth: f32,
) -> Option<Vec<f32>> {
    let (graph, node) = (graph?, node?);
    let edge = graph.edges.iter().find(|e| e.to.node == node && e.to.port == "control")?;
    let from = graph.node(&edge.from.node)?;
    if !from.kind.starts_with("ai.") {
        return None;
    }
    // 2026-10-01: `mode: "auto"` is the operator choosing the deterministic
    // path on purpose — the round robin `drive::row`, plain math over the
    // live digest, no fitted weights — without unwiring the node.
    if ai_mode(from) == AiMode::Auto {
        return None;
    }
    let weights = ai_weights(state, from, schema, model)?;
    let music = drive::blended(frame, smooth);
    let mut logic = metal_logic::ColouristLogic::new(weights);
    let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
    let out = run_with_budget(&mut logic, &AuxInput::new(&music), &ctx).ok()?;
    Some(out.row)
}

// ---------------------------------------------------------------------------
// H3: head tracking -> the reserved camera floats
// ---------------------------------------------------------------------------

/// The three floats a `cam.track` node writes, as an [`AuxLogic`] row — the
/// same declaration `metal_sense`'s camera offset produces. Runs through
/// [`run_with_budget`] exactly like the Colourist, so a slow or bad sensor
/// reading is a value (`None`), never a panic and never a dropped frame.
static HEAD_FLOATS: [FloatDecl; 3] = [
    FloatDecl::new("cam_x", -3.0, 3.0, 0.0),
    FloatDecl::new("cam_y", -3.0, 3.0, 0.0),
    FloatDecl::new("cam_z", -3.0, 3.0, 0.0),
];

/// The `cam.track` runtime: a normalized eyes reading in, a camera offset out.
struct HeadLogic {
    strength: f32,
    invert: [bool; 3],
}

impl AuxLogic for HeadLogic {
    fn id(&self) -> &'static str {
        "cam.track"
    }

    fn floats(&self) -> &'static [FloatDecl] {
        &HEAD_FLOATS
    }

    fn input_width(&self) -> usize {
        3
    }

    fn run(&mut self, input: &AuxInput, _ctx: &LogicContext) -> Result<AuxOutput, AuxError> {
        if input.row.len() < 3 {
            return Err(AuxError::BadInput(format!(
                "cam.track expects eye_x/eye_y/eye_z_est, got {} floats",
                input.row.len()
            )));
        }
        let control = metal_sense::Control::new([
            ("eye_x", input.row[0]),
            ("eye_y", input.row[1]),
            ("eye_z_est", input.row[2]),
        ]);
        let mut off = metal_sense::eyes_to_camera_offset(&control, metal_sense::DEFAULT_EYE_SENSITIVITY);
        for (k, v) in off.iter_mut().enumerate() {
            if self.invert[k] {
                *v = -*v;
            }
            *v *= self.strength;
        }
        Ok(AuxOutput::new(off.to_vec()))
    }
}

/// The camera offset one `cam.track` node's config wants from a sensor reading,
/// or `None` when the reading has no channels (a stale `Control::zeroed`, which
/// must mean "no offset", not "face pinned at a corner").
pub fn head_offset(
    control: &metal_sense::Control,
    config: &metal_graph::camera_track::CamTrackConfig,
) -> Option<[f32; 3]> {
    let row = [
        control.get("eye_x")?,
        control.get("eye_y")?,
        control.get("eye_z_est")?,
    ];
    let mut logic = HeadLogic { strength: config.strength, invert: config.invert };
    let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
    let out = run_with_budget(&mut logic, &AuxInput::new(&row), &ctx).ok()?;
    Some([out.row[0], out.row[1], out.row[2]])
}

/// The resolution `sensor.yolo` detectors read at — `the design notes` § H0's measured
/// 640×360, the size the bench runs.
pub const HEAD_DETECT_W: u32 = 640;
pub const HEAD_DETECT_H: u32 = 360;

/// H3: one frame's head sensing. For every `sensor.yolo` node, read the frame
/// its upstream `capture` node decoded (or a test's stand-in in
/// `state.head_frames`), run the node's detector, and update `state.head` —
/// the per-node eyes reading `cam.track` consumes.
///
/// **Budgeted, error as a value.** A `capture` with no frame yet, a detector
/// that finds nothing, and a call that overruns [`Budget::DEFAULT`] all read
/// the same way: the node's previous reading is held (marked stale) rather
/// than a zero invented for it, and nothing panics. Returns how many sensors
/// ran.
pub fn poll_head_sensors(graph: &metal_graph::Graph, state: &mut State, w: u32, h: u32) -> usize {
    let mut ran = 0;
    for sensor in graph.nodes.iter().filter(|n| n.kind == "sensor.yolo") {
        let Some(capture) = graph
            .edges
            .iter()
            .find(|e| e.to.node == sensor.id && e.to.port == "image")
            .map(|e| e.from.node.clone())
        else {
            continue;
        };
        let image = head_frame(graph, state, &capture, w, h);
        // H5: the node's smoothing dials. A change rebuilds the detector, so
        // the config is read, never a second state that does nothing.
        let smoothing = sensor_smoothing(graph.node(&sensor.id));
        if state.head_smoothing.get(&sensor.id).is_some_and(|prev| *prev != smoothing) {
            state.head_detectors.remove(&sensor.id);
        }
        let detector = state
            .head_detectors
            .entry(sensor.id.clone())
            .or_insert_with(|| make_head_detector(metal_sense::Mode::Eyes, smoothing.0, smoothing.1));
        state.head_smoothing.insert(sensor.id.clone(), smoothing);
        let fresh = image.as_ref().and_then(|img| {
            let start = Instant::now();
            let out = detector.detect(img);
            if Budget::DEFAULT.exceeded_by(start.elapsed()) {
                None
            } else {
                out
            }
        });
        match fresh {
            Some(c) => {
                state.head.insert(sensor.id.clone(), c);
            }
            None => {
                if let Some(prev) = state.head.get_mut(&sensor.id) {
                    prev.update(None);
                }
            }
        }
        ran += 1;
    }
    ran
}

/// The frame a `sensor.yolo` node reads this frame: a test's stand-in for the
/// capture node if one was handed in, else the device's newest frame at
/// `w × h`. `None` when the node upstream is not a `capture`, or nothing has
/// landed yet.
fn head_frame(
    graph: &metal_graph::Graph,
    state: &State,
    capture: &metal_graph::NodeId,
    w: u32,
    h: u32,
) -> Option<metal_sense::Image> {
    if let Some(img) = state.head_frames.get(capture) {
        return Some(img.clone());
    }
    let node = graph.node(capture)?;
    if node.kind != "capture" {
        return None;
    }
    let source: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    let src = source["source"].as_str()?;
    let px = crate::capture::frame(src, w, h)?;
    Some(metal_sense::Image::new(w, h, px))
}

/// The detector a `sensor.yolo` node runs. A real backend behind `metal-sense`'s
/// `mediapipe` feature (weights from `scripts/setup-vision`); otherwise a
/// silent stand-in that reads nothing, which `cam.track` correctly treats as
/// no offset rather than as a face pinned at a corner.
fn make_head_detector(mode: metal_sense::Mode, min_cutoff: f32, beta: f32) -> Box<dyn metal_sense::Detector + Send> {
    #[cfg(feature = "vision")]
    {
        let dir = crate::output::data_root().join("models");
        if let Ok(mut d) = metal_sense::MediaPipeDetector::try_load(mode, &dir.to_string_lossy()) {
            d.set_smoothing(min_cutoff, beta);
            return Box::new(d);
        }
    }
    let _ = (min_cutoff, beta);
    Box::new(metal_sense::StandInDetector::new(mode, Vec::new()))
}

/// A `sensor.yolo` node's `min_cutoff`/`beta` off its config, defaulting to
/// the same dials the properties panel shows (`1.0`, `0.5`).
fn sensor_smoothing(node: Option<&metal_graph::Node>) -> (f32, f32) {
    let Some(node) = node else { return (1.0, 0.5) };
    let v: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or_default();
    (
        v["min_cutoff"].as_f64().unwrap_or(1.0) as f32,
        v["beta"].as_f64().unwrap_or(0.5) as f32,
    )
}

/// The reserved `cam_x`/`cam_y`/`cam_z` positions of a model in the manifest,
/// by name — read off `data/schema.json` so the engine needs no GPU schema.
/// `None` for a model that does not declare the block.
pub fn camera_track_indices(model: &metal_schema::ManifestModel) -> Option<[usize; 3]> {
    let mut found: [Option<usize>; 3] = [None, None, None];
    let mut at = 0usize;
    for block in &model.blocks {
        match block {
            metal_schema::ManifestBlock::Floats { fields } => {
                for (i, f) in fields.iter().enumerate() {
                    match f.name.as_str() {
                        "cam_x" => found[0] = Some(at + i),
                        "cam_y" => found[1] = Some(at + i),
                        "cam_z" => found[2] = Some(at + i),
                        _ => {}
                    }
                }
                at += fields.len();
            }
            metal_schema::ManifestBlock::ColorSlot { .. } => at += 12,
            metal_schema::ManifestBlock::Camera { .. } => at += 7,
        }
    }
    match found {
        [Some(x), Some(y), Some(z)] => Some([x, y, z]),
        _ => None,
    }
}

/// Sums every `cam.track` node's viewer offset into the reserved camera floats
/// of its targets' rows, on top of whatever the sequence and the `ai.*` nodes
/// already put there. `sensors` is the per-node eyes reading (`State::head`);
/// a `cam.track` whose `control` port is unwired, or wired to a node with no
/// reading, writes nothing. Returns how many target rows moved.
pub fn apply_camera_track(
    graph: &metal_graph::Graph,
    rows: &mut HashMap<metal_graph::NodeId, Vec<f32>>,
    manifest: &metal_schema::Manifest,
    sensors: &HashMap<metal_graph::NodeId, metal_sense::Control>,
) -> usize {
    let mut moved = 0;
    for node in graph.nodes.iter().filter(|n| n.kind == "cam.track") {
        let config: metal_graph::camera_track::CamTrackConfig =
            serde_json::from_str(&node.config.to_json_str()).unwrap_or_default();
        // The eyes reading arrives on this node's `control` port — the
        // `sensor.yolo` wired into it. Unwired or unread: nothing to add.
        let control = graph
            .edges
            .iter()
            .find(|e| e.to.node == node.id && e.to.port == "control")
            .and_then(|e| sensors.get(&e.from.node));
        let Some(offset) = control.and_then(|c| head_offset(c, &config)) else { continue };
        // An empty `targets` is the operator's "all of them": every generator
        // in the graph whose model declares `camera_track()`. A non-empty list
        // is honoured as written (invalid entries dropped by `targets`).
        let mut resolved = metal_graph::camera_track::targets(graph, &node.id, manifest);
        if resolved.is_empty() && config.targets.is_empty() {
            resolved = graph
                .nodes
                .iter()
                .filter(|n| {
                    n.kind
                        .strip_prefix("gen.")
                        .is_some_and(|id| manifest.models.iter().any(|m| m.id == id && m.camera_track))
                })
                .map(|n| n.id.clone())
                .collect();
        }
        for target in resolved {
            let Some(model_id) = graph.node(&target).and_then(|n| n.kind.strip_prefix("gen.")) else {
                continue;
            };
            let Some(indices) = manifest
                .models
                .iter()
                .find(|m| m.id == model_id)
                .and_then(camera_track_indices)
            else {
                continue;
            };
            let Some(row) = rows.get_mut(&target) else { continue };
            for (k, at) in indices.iter().enumerate() {
                if let Some(v) = row.get_mut(*at) {
                    *v += offset[k];
                }
            }
            moved += 1;
        }
    }
    moved
}

/// The manifest `cam.track` resolves its targets against — `data/schema.json`,
/// cached for the process (a suite does not gain or lose a model mid-set), with
/// the build's own copy as the fallback. Returns `'static` so the per-frame
/// call borrows it instead of cloning 300 kB.
pub(crate) fn shared_manifest() -> &'static metal_schema::Manifest {
    static CACHE: std::sync::OnceLock<metal_schema::Manifest> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        metal_vfs::read_to_string(output::data_root().join("schema.json"))
            .ok()
            .and_then(|s| metal_schema::Manifest::parse(&s).ok())
            .or_else(|| metal_schema::Manifest::parse(include_str!("../../../data/schema.json")).ok())
            .unwrap_or(metal_schema::Manifest { rate: 8.0, models: Vec::new() })
    })
}

/// How an `ai.*` node drives its source: `AiAuto` (the default) applies the
/// fitted grade when there is one; `Auto` is the deterministic round robin
/// even when a grade exists. Config key `mode`, `"ai"` | `"auto"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiMode {
    AiAuto,
    Auto,
}

/// How a `gen.*` source drives its own floats (2026-10-08). One field on the
/// source — config key `mode`, `"ai" | "ai_fx" | "manual"` — replaces the
/// split between "is an `ai.*` node wired" and the `drive` dials, so the
/// colourist no longer needs a visual node of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modo {
    /// The fitted colourist's offsets (today's `AiMode::AiAuto` with a
    /// wired, trained `ai.*`).
    Ai,
    /// The same as [`Modo::Ai`], plus the effect floats the node embeds in
    /// its own config (`config.effects`), driven by the digest too — see
    /// [`embedded_effects`]. The source's own row is identical to `ai`'s.
    AiFx,
    /// The operator's held row, no audio drive at all.
    Manual,
    /// No `mode` key: the pre-`modo` behaviour (fitted grade when one is
    /// wired and trained, else the round robin). A node saved before the key
    /// existed keeps working unchanged.
    Inherit,
}

/// The `modo` a `gen.*` node's config asks for. Only the three explicit
/// values are recognised; anything else (including a missing key, or an
/// `ai.*` node whose own `mode` is `"ai"/"auto"`) reads as [`Modo::Inherit`],
/// so nothing changes for a graph saved before this key existed.
pub fn node_modo(node: Option<&metal_graph::Node>) -> Modo {
    let Some(node) = node else { return Modo::Inherit };
    let config: Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null);
    match config.get("mode").and_then(Value::as_str) {
        Some("ai") => Modo::Ai,
        Some("ai_fx") => Modo::AiFx,
        Some("manual") => Modo::Manual,
        _ => Modo::Inherit,
    }
}

/// The digest-driven values of the effect floats a `gen.*` node embeds in its
/// own config (`config.effects`, plan §4), in the config's stable order, or
/// empty unless the node's `modo` is [`Modo::AiFx`].
///
/// `ai_fx` is `ai` **plus** the effects the node carries itself: [`row_for_node`]
/// drives the source's own floats on the `ai` path for both, and this is what
/// the `ai_fx` path adds. Each value is driven exactly like a source float
/// ([`drive::embedded_effect_floats`] + [`drive::drive_effect_float`]), so the
/// behaviour is defined and testable for any name; the paint/render side reads
/// it rather than a new render pass. `ai`, `manual` and `inherit` carry no
/// embedded effects.
pub fn embedded_effects(
    node: Option<&metal_graph::Node>,
    frame: &digest::Frame,
    drive: &Drive,
) -> Vec<(String, f32)> {
    if node_modo(node) != Modo::AiFx {
        return Vec::new();
    }
    drive::embedded_effect_floats(node)
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            let column = drive::effect_float_column(&f.name, i);
            let value = drive::drive_effect_float(&f, column, frame, drive);
            (f.name, value)
        })
        .collect()
}

/// The `mode` an `ai.*` node's config asks for; anything but `"auto"` reads
/// as [`AiMode::AiAuto`], so a node saved before the key existed keeps its
/// behaviour.
pub fn ai_mode(node: &metal_graph::Node) -> AiMode {
    let config: Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null);
    match config.get("mode").and_then(Value::as_str) {
        Some("auto") => AiMode::Auto,
        _ => AiMode::AiAuto,
    }
}

/// The fitted rows an `ai.colourist` node should apply, or `None` when its
/// model is untrained.
///
/// The node's config is `{ "weights": "<name>" | null }`. `null` uses the
/// downstream model's own default (`state.reels.colourist`, cached per model),
/// which is the file the studio's train button writes. A name loads a grade
/// from the same directory `Colourist::model_path` uses — the plan says
/// `data/grade/`, but the code's one colourist directory is `data/reels/`
/// (`metal_reel::index::DIR`), so a named weights file is
/// `<data_root>/reels/<name>.json`. A grade fitted for another model is
/// refused by [`metal_reel::grade::Grade::fits`], which keeps a stale file from
/// silently grading the wrong parameters.
fn ai_weights(
    state: &mut State,
    node: &metal_graph::Node,
    schema: &metal_schema::Schema,
    model: &str,
) -> Option<Vec<Vec<f32>>> {
    let config: Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null);
    let named = config.get("weights").and_then(Value::as_str).filter(|n| !n.is_empty());
    match named {
        None => {
            let colourist = state.reels.colourist(model, schema);
            if !colourist.trained() {
                return None;
            }
            Some(colourist.grade().map(|g| g.weights(metal_schema::TickRate::Base).clone()).unwrap_or_default())
        }
        Some(name) => {
            let path = output::data_root()
                .join(metal_reel::index::DIR)
                .join(format!("{name}.json"));
            let text = metal_vfs::read_to_string(path).ok()?;
            let grade: metal_reel::grade::Grade = serde_json::from_str(&text).ok()?;
            grade.fits(model, schema).then(|| grade.weights(metal_schema::TickRate::Base).clone())
        }
    }
}

/// One source's own picture, for the card it lives on.
///
/// Deliberately **not** its sector, its contour, its opacity or its blend mode:
/// this is the animation as the model draws it, so the thumbnail answers "what
/// is this source" rather than "what does this source contribute to that
/// device", which the device's own thumbnail already answers. It is also what
/// makes the picture cheap enough to run one per card — a card is 160px wide and
/// this is one `solo` at that size.
///
/// **`R22.1`: a layer that exists only as a node of the device's authored
/// graph.** The pool is the routing table, and a `gen.*` added in the canvas
/// that the graph composes may never have been routed there — so when the
/// pool has no such layer, the same walk [`graph_composed_layers`] makes
/// finds it: the graph named after the device first, then a graph whose own
/// id is `device` (the canvas can be showing a graph that is not a
/// `device-*` one). The found layer renders exactly like a pool layer —
/// node config, reaction and all.
pub fn render_source(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    layer: &str,
    w: u32,
    h: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    match render_source_picture(paint, sim, state, device, layer, w, h, false)? {
        NodePicture::Cpu(px, pw, ph) => Some((px, pw, ph)),
        NodePicture::Gpu(f) => Some((paint.readback(&f), f.width(), f.height())),
    }
}

/// [`render_source`] as a texture on the painter's device (G5): `None` when
/// the picture is pixels (a CPU painter). The caller then asks the JPEG route.
pub fn render_source_texture(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    layer: &str,
    w: u32,
    h: u32,
) -> Option<crate::gpu_frame::GpuFrame> {
    match render_source_picture(paint, sim, state, device, layer, w, h, true)? {
        NodePicture::Gpu(f) => Some(f),
        NodePicture::Cpu(px, pw, ph) => upload_picture(paint, px, pw, ph),
    }
}

/// The body of [`render_source`]; `textures` asks the solo draw for a GPU
/// texture rather than pixels.
#[allow(clippy::too_many_arguments)]
fn render_source_picture(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    layer: &str,
    w: u32,
    h: u32,
    textures: bool,
) -> Option<NodePicture> {
    let time = state.time();
    // Same frame the router routed on, taken from the one Looper — see
    // `render_device`.
    let frame = sim.clock_frame(&mut state.looper, time);
    let air = state.airing(sim, time);
    let models = paint.model_count();

    let pool: Vec<Layer> = state.routed_to(device).into_iter().cloned().collect();
    let mut found = pool.iter().find(|l| l.id == layer).cloned();
    if found.is_none() {
        let root = output::data_root();
        let ids = paint.ids();
        let graph = preview_graph_at(&root, device);
        found = graph.as_ref().and_then(|g| {
            graph_composed_layers(g, &pool, &ids)
                .and_then(|list| list.into_iter().find(|l| l.id == layer))
                // A generator no output composes (not wired yet, or a sibling
                // gen of another kind spoils the whole walk) is still a node
                // of the graph: its own picture is what the focus asks for.
                .or_else(|| standalone_gen_layer(g, layer, &pool, &ids))
        });
    }
    let mut found = found?;
    // §16.7. The card's own picture has to show the movement too: a trigger
    // that changed the wall and not the thumbnail the operator is looking at
    // would read as a trigger that did nothing.
    state
        .overlays
        .on(device, time)
        .apply(&air, models, std::slice::from_mut(&mut found));
    let found = found;
    let showing = found.showing(&air, models);
    let base: Vec<f32> = found
        .base
        .clone()
        .unwrap_or_else(|| state.base[showing.min(state.base.len() - 1)].clone());
    let passage = state.router.content.passage();
    // The card's own picture is a thumbnail of one source, not a device
    // render: it has no graph in hand (the one it just walked was consumed by
    // `graph_composed_layers`), so it draws with the round robin. The live
    // render is `render_device`, and that is where an `ai.*` node is read.
    let mut row = row_for(
        paint,
        state,
        showing,
        &base,
        &frame,
        time,
        passage,
        found.library.as_deref(),
    );
    apply_node_post(&mut row, &found.effects, found.fijo_mask);
    if textures {
        if let Some(f) = paint.solo_texture(showing, &row, time, w, h, found.fondo_alpha, &[]) {
            return Some(NodePicture::Gpu(f));
        }
    }
    let (px, pw, ph) = paint.solo(showing, &row, time, w, h, found.fondo_alpha);
    Some(NodePicture::Cpu(px, pw, ph))
}

/// The id [`isolate_graph`] gives the stand-in `output.device` it wires the
/// isolated node into.
pub const ISOLATED_OUT: &str = "__isolated_out";

/// The graph cut down to "everything that makes `at`'s picture, and nothing
/// after it": `at` plus every node up its `image` wires, with every `image`
/// edge leaving that set dropped and a stand-in `output.device`
/// ([`ISOLATED_OUT`]) fed by `at` instead. Every other edge (control,
/// `param:*` bindings) stays, so the rows are driven exactly as live.
///
/// Why a cut graph and not a buffer read: an `fx.*` chain is baked into the
/// generator's own shader pass (`crate::resolved_chain` walks *downstream*
/// of the generator), so a node's buffer in the full graph already carries
/// every effect after it. Cutting the graph at `at` is what makes the chain
/// the renderer resolves end there — through the one render path
/// ([`Paint::run_with_clocks`]), not a second one. `None` if `at` is not in
/// the graph.
pub fn isolate_graph(graph: &metal_graph::Graph, at: &str) -> Option<metal_graph::Graph> {
    let node = graph.node(at)?.clone();
    let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stack = vec![at.to_string()];
    while let Some(cur) = stack.pop() {
        if !keep.insert(cur.clone()) {
            continue;
        }
        for e in graph.edges.iter().filter(|e| e.to.node == cur && e.to.port == "image") {
            stack.push(e.from.node.clone());
        }
    }
    let out_port = graph
        .edges
        .iter()
        .find(|e| e.from.node == at && e.to.port == "image")
        .map(|e| e.from.port.clone())
        .unwrap_or_else(|| "image".to_string());
    let mut sub = graph.clone();
    sub.edges
        .retain(|e| !(keep.contains(&e.from.node) && e.to.port == "image" && !keep.contains(&e.to.node)));
    // Other sinks would pull their own chains into the plan; only the
    // stand-in output may consume anything.
    sub.nodes.retain(|n| n.kind != "output.device");
    sub.edges.retain(|e| sub.nodes.iter().any(|n| n.id == e.to.node));
    let mut out = node;
    out.id = ISOLATED_OUT.to_string();
    out.kind = "output.device".to_string();
    out.bypass = false;
    out.solo = false;
    out.config = Default::default();
    sub.nodes.push(out);
    sub.edges.push(metal_graph::Edge::new(
        "__isolated_edge",
        metal_graph::Endpoint { node: at.to_string(), port: out_port },
        metal_graph::Endpoint { node: ISOLATED_OUT.to_string(), port: "image".to_string() },
    ));
    Some(sub)
}

/// One node's own picture, for isolating a stage while debugging — what
/// `GET /node.jpg` answers.
///
/// - `output.*` / `compose.*`: the device's composed frame, the very
///   [`render_device`] `/output.jpg` draws.
/// - `mask`: that same frame cropped to the mask's bounds — the one part of
///   the output this mask renders.
/// - anything else (`gen.*`, `fx.*`, text): the picture *right after* that
///   node — the generator with only the effects up to and including it, cut
///   by [`isolate_graph`] and drawn by [`Paint::run_with_clocks`], rows
///   driven and bound the way `render_device` drives them.
///
/// `None` when the device has no graph, the node is not in it, or nothing
/// upstream of it draws.
pub fn render_node(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    node: &str,
    w: u32,
    h: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    match render_node_picture(paint, sim, state, device, node, w, h, false)? {
        NodePicture::Cpu(px, pw, ph) => Some((px, pw, ph)),
        NodePicture::Gpu(f) => Some((paint.readback(&f), f.width(), f.height())),
    }
}

/// A node's picture as the painter left it: pixels, or a texture on the
/// painter's device when the node's buffer stayed one (`textures` on).
pub enum NodePicture {
    Gpu(crate::gpu_frame::GpuFrame),
    Cpu(Vec<u8>, u32, u32),
}

/// [`render_node`] as a texture on the shared device (G4, G5): a picture the
/// painter kept on the GPU is returned as it is; one that came out as pixels
/// (a mask or device node, a buffer that needed a readback) is uploaded. `None`
/// only on a CPU painter, which keeps the JPEG route.
pub fn render_node_texture(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    node: &str,
    w: u32,
    h: u32,
) -> Option<crate::gpu_frame::GpuFrame> {
    match render_node_picture(paint, sim, state, device, node, w, h, true)? {
        NodePicture::Gpu(f) => Some(f),
        // G5: a picture the painter had to read back is uploaded again, so the
        // window still gets a texture and no JPEG is encoded.
        NodePicture::Cpu(px, pw, ph) => upload_picture(paint, px, pw, ph),
    }
}

/// A picture the painter read back, uploaded again for a texture consumer
/// (G5). Without the `gpu` feature there is no texture to make: `None`.
#[cfg(feature = "gpu")]
fn upload_picture(paint: &mut dyn Paint, px: Vec<u8>, w: u32, h: u32) -> Option<crate::gpu_frame::GpuFrame> {
    paint.gpu_cx().map(|cx| cx.upload(&px, w, h))
}

#[cfg(not(feature = "gpu"))]
fn upload_picture(_: &mut dyn Paint, _: Vec<u8>, _: u32, _: u32) -> Option<crate::gpu_frame::GpuFrame> {
    None
}

/// The body of [`render_node`]; `textures` asks the tail for a GPU buffer.
#[allow(clippy::too_many_arguments)]
fn render_node_picture(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    node: &str,
    w: u32,
    h: u32,
    textures: bool,
) -> Option<NodePicture> {
    let root = output::data_root();
    let graph = preview_graph_at(&root, device)?;
    let target = graph.node(node)?.clone();
    if target.kind.starts_with("output.") || metal_graph::is_compose_kind(&target.kind) {
        let (px, pw, ph) = render_device(paint, sim, state, device, w, h);
        return Some(NodePicture::Cpu(px, pw, ph));
    }
    if let Some(mask_id) = mask_node_id(&target) {
        let full = render_device(paint, sim, state, device, w, h);
        let masks: Vec<Mask> = state.outputs.find(device).map(|o| o.masks.clone()).unwrap_or_default();
        let bounds = masks.iter().find(|m| m.id == mask_id).filter(|m| m.closed())?.bounds_in(&masks);
        let (px, pw, ph) = crop_window(&full.0, full.1, full.2, &bounds).unwrap_or(full);
        return Some(NodePicture::Cpu(px, pw, ph));
    }

    let time = state.time();
    render_isolated(paint, sim, state, device, graph, node, w, h, time, textures)
}

/// The isolated stage of `node` in `graph` at clock `time`: [`render_node`]'s
/// body once the graph is in hand, so a graph that is not on disk (a kind's
/// own preview, [`render_kind`]) renders through the same path as a device's.
/// `device` keys the clocks and nothing else; the output node is not drawn here.
#[allow(clippy::too_many_arguments)]
pub fn render_isolated(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    device: &str,
    graph: metal_graph::Graph,
    node: &str,
    w: u32,
    h: u32,
    time: f32,
    textures: bool,
) -> Option<NodePicture> {
    let mut sub = isolate_graph(&graph, node)?;
    let plan = sub.compile();
    let clock = sim.clock_frame(&mut state.looper, time);
    let digests = sim.digests(&sub, clock, time);
    let frame = digests.graph_frame(&sub);
    let passage = state.router.content.passage();
    let manifest_ids = paint.ids();
    // The same clock the device pass drives its generators with (`render_device_frame`):
    // `advance` is idempotent at one instant, so asking here cannot move it.
    let gen_clock = state.clocks.advance(device, &graph, time);
    // The same layer the device pass resolves for each generator (an authored
    // canvas composes its own layers via `graph_composed_layers`). Its base row,
    // library and per-layer post are then the device's, not a second guess.
    let air = state.airing(sim, time);
    let models = paint.model_count();
    let composed: HashMap<metal_graph::NodeId, Layer> =
        if resolve_authored_mode(&graph) == Some(layers::DeviceMode::Own) {
            let gens_in_order: Vec<metal_graph::NodeId> = authored_generators_with_bindings(&graph)
                .into_iter()
                .map(|(id, _, _)| id)
                .filter(|id| !graph.node(id).is_some_and(|n| n.bypass))
                .collect();
            graph_composed_layers(&graph, &state.layers, &manifest_ids)
                .map(|list| gens_in_order.into_iter().zip(list).collect())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
    let mut rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> = std::collections::HashMap::new();
    let mut fondo_alpha: std::collections::HashMap<metal_graph::NodeId, bool> = std::collections::HashMap::new();
    let gens: Vec<(String, usize)> = sub
        .nodes
        .iter()
        .filter_map(|n| {
            let model = n.kind.strip_prefix("gen.")?;
            Some((n.id.clone(), manifest_ids.iter().position(|id| *id == model)?))
        })
        .collect();
    for (gen_id, showing) in gens {
        // The layer the device would draw for this generator; a generator outside
        // the authored canvas (no device layer) is driven from its own config.
        let layer = match composed.get(&gen_id) {
            Some(l) => l.clone(),
            None => {
                let mut l = Layer::new(&gen_id, &gen_id, showing);
                if let Some(n) = graph.node(&gen_id) {
                    node_layer::NodeLayerConfig::from_node(n).apply_to(&mut l);
                }
                l
            }
        };
        let showing = layer.showing(&air, models);
        let base: Vec<f32> = layer
            .base
            .clone()
            .unwrap_or_else(|| state.base[showing.min(state.base.len() - 1)].clone());
        let wired = digests.wired(&sub, &gen_id);
        let mut row = row_for_node(
            Some(&graph),
            Some(&gen_id),
            paint,
            state,
            showing,
            &base,
            wired,
            time,
            passage,
            layer.library.as_deref(),
        );
        if manifest_ids.get(showing).copied() == Some(crate::forge::MODEL) {
            crate::forge::ingest_row(&mut row, wired.unwrap_or(&Digests::zero()));
        }
        apply_node_post(&mut row, &layer.effects, layer.fijo_mask);
        fondo_alpha.insert(gen_id.clone(), layer.fondo_alpha);
        rows.insert(gen_id, row);
    }
    let gen_params = crate::bind::gen_params(&sub, &manifest_ids, &paint.schemas());
    let repeat = {
        let s = state.looper.status();
        crate::bind::RepeatVars { locked: if s.engaged.is_some() { 1.0 } else { 0.0 }, phase: s.phase }
    };
    apply_effect_loops(&mut sub, &mut rows, &state.loops, state.music_pos);
    crate::bind::before_scripts(&mut sub, &plan, &mut rows, &gen_params, &frame.raw, &repeat);
    let scripts = crate::script::evaluate_clocked(&sub, &rows, &frame.raw, &gen_params, script_clock(state, time));
    crate::bind::after_scripts(&mut sub, &plan, &mut rows, &scripts.rows, &gen_params, &frame.raw, &repeat);
    if sub.nodes.iter().any(|n| n.kind == "sensor.yolo") {
        poll_head_sensors(&sub, state, HEAD_DETECT_W, HEAD_DETECT_H);
    }
    if sub.nodes.iter().any(|n| n.kind == "cam.track") {
        apply_camera_track(&sub, &mut rows, shared_manifest(), &state.head);
    }
    stamp_beats(&mut sub, sim, time);
    let result = paint.run_full(
        &sub,
        &plan,
        &rows,
        &fondo_alpha,
        &frame.raw,
        time,
        w,
        h,
        &gen_clock,
        &std::collections::HashMap::new(),
        textures,
    );
    let buf = plan.ops.iter().find(|op| op.node == node)?.buffer?;
    if let Some(f) = result.textures.get(&buf) {
        return Some(NodePicture::Gpu(f.clone()));
    }
    let (px, pw, ph) = result.frames.get(&buf).cloned()?;
    Some(NodePicture::Cpu(px, pw, ph))
}

/// The points a kind's card variants sit at, as fractions of each float's
/// own `min..max`: four distinct points, none on an edge (the same spread the
/// model cards use).
pub const VARIANT_T: [f32; 4] = [0.15, 0.4, 0.65, 0.9];

/// The device id a kind preview keys its clocks under.
pub const KIND_PREVIEW_DEVICE: &str = "kind-preview";

/// The one clock instant every kind preview is drawn at, so the four variants
/// differ only in their floats and a card does not change between fetches.
pub const KIND_PREVIEW_TIME: f32 = 3.0;

/// The floats a kind's variant `variant` sets, as `(config key, value)`: every
/// float a `fx.*` kind exposes (its catalogue params), or every `mat.*` config
/// key that has a row range. Empty for any other kind.
pub fn kind_variant_floats(kind: &str, variant: usize) -> Vec<(String, f32)> {
    let t = VARIANT_T[variant % VARIANT_T.len()];
    let at = |min: f32, max: f32| min + (max - min) * t;
    if metal_graph::materials::is_material_kind(kind) {
        return metal_graph::materials::CONFIG_DEFAULTS
            .iter()
            .filter_map(|(key, _)| {
                let (min, max) = metal_graph::materials::range_of(key)?;
                Some((key.to_string(), at(min, max)))
            })
            .collect();
    }
    metal_graph::params::effect_params(kind)
        .into_iter()
        .map(|p| (p.name.clone(), at(p.min, p.max)))
        .collect()
}

/// The test model an fx preview draws over. A manifest model (not `gen.ramp`,
/// which is a tile source the isolated stage does not drive), fixed so the
/// same frame comes back every time.
pub const KIND_FX_SOURCE: &str = "gen.rings";
/// The 3D test model a material preview wears its material on.
pub const KIND_MAT_SOURCE: &str = "gen.scene3d";

/// The stand-in graph a kind's preview renders: the test model `model` into
/// the kind, the kind's floats set to variant `variant`'s. [`render_kind`]
/// picks the source ([`KIND_FX_SOURCE`] or [`KIND_MAT_SOURCE`]). `None` for a
/// kind with no preview.
pub fn kind_preview_graph(kind: &str, variant: usize, model: &str) -> Option<(metal_graph::Graph, metal_graph::NodeId)> {
    if !metal_graph::materials::is_material_kind(kind) && !kind.starts_with("fx.") {
        return None;
    }
    let floats = kind_variant_floats(kind, variant);
    if floats.is_empty() {
        return None;
    }
    let config: serde_json::Map<String, serde_json::Value> =
        floats.into_iter().map(|(k, v)| (k, serde_json::json!(v))).collect();
    let config = metal_graph::Config::from_json_str(&serde_json::Value::Object(config).to_string()).ok()?;
    let mut g = metal_graph::Graph::new(&device_graph_id(KIND_PREVIEW_DEVICE), KIND_PREVIEW_DEVICE, metal_graph::Domain::Live);
    let out = g.add_node("out", "output.device", [800.0, 0.0]);
    let src = g.add_node("src", &format!("gen.{model}"), [0.0, 0.0]);
    let node = g.add_node("kind", kind, [400.0, 0.0]);
    g.node_mut(&node)?.config = config;
    g.add_edge(metal_graph::Endpoint::new(&src, "image"), metal_graph::Endpoint::new(&node, "image"));
    g.add_edge(metal_graph::Endpoint::new(&node, "image"), metal_graph::Endpoint::new(&out, "image"));
    Some((g, node))
}

/// `kind`'s picture at variant `variant`: [`kind_preview_graph`] cut at the
/// kind and drawn at [`KIND_PREVIEW_TIME`] through [`render_isolated`]. `None`
/// for a kind with no preview, or when the painter draws nothing.
pub fn render_kind(
    paint: &mut dyn Paint,
    sim: &mut Listeners,
    state: &mut State,
    kind: &str,
    variant: usize,
    w: u32,
    h: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    let ids = paint.ids();
    let wanted = if metal_graph::materials::is_material_kind(kind) { KIND_MAT_SOURCE } else { KIND_FX_SOURCE };
    // The wanted test model when this painter has it, else the first model it has.
    let wanted = wanted.strip_prefix("gen.").unwrap_or(wanted);
    let source = ids.iter().copied().find(|id| *id == wanted).or(ids.first().copied())?;
    let (graph, node) = kind_preview_graph(kind, variant, source)?;
    match render_isolated(paint, sim, state, KIND_PREVIEW_DEVICE, graph, &node, w, h, KIND_PREVIEW_TIME, false)? {
        NodePicture::Cpu(px, pw, ph) => Some((px, pw, ph)),
        NodePicture::Gpu(f) => Some((paint.readback(&f), f.width(), f.height())),
    }
}

/// Applies this layer's own fixed effects on top of a row already driven by
/// `drive::row` from the digest.
///
/// The row's trailing floats are `shake`/`light_leak`/`ghost`/`kaleido`/…/
/// `leak_color` as ordinary floats — `EFFECTS_FLOATS_BLOCK` and
/// `EFFECTS_COLOR_BLOCK`, appended to every model's schema so the studio can
/// keyframe them per tick. Before the design notes PARTE IV §17.4 Conjunto E, this
/// whole block was overwritten unconditionally by the app's one master dial
/// (`apply_master_post`, long gone — every remaining call site has a `Layer`
/// to read a mask from). This is the replacement: it only overwrites what `fijo_mask` marks as fixed for
/// *this* layer, leaving every other float exactly as `drive::row` computed
/// it. That is what lets two layers sharing one generator disagree about a
/// filter — each carries its own `effects`/`fijo_mask` rather than the app
/// carrying one `Post` for everyone.
///
/// `fijo_mask` bit *i* set means one of the **14 common** effects
/// (`EFFECTS_FLOATS`'s first `CHAIN_FLOATS` entries, in order), then bit 14
/// for the `leak_color` colour slot, is fixed to `post`'s value. The advanced
/// effects are append-only and past the mask's vocabulary: this legacy
/// per-layer mask predates them, so it neither fixes nor reaches them. `0`,
/// what every stack saved before this field existed becomes via
/// `#[serde(default)]`, means nothing is fixed: the row is left exactly as
/// driven. Whether that reads as "reactive" or "off" is not a distinction
/// this function makes — see [`crate::layers::Layer::fijo_mask`].
pub fn apply_node_post(row: &mut [f32], post: &Post, fijo_mask: u32) {
    if row.len() < metal_schema::EFFECTS_WIDTH {
        return;
    }
    let at = row.len() - metal_schema::EFFECTS_WIDTH;

    // Bit positions match EFFECTS_FLOATS order
    if (fijo_mask & (1 << 0)) != 0 {
        row[at] = post.shake;
    }
    if (fijo_mask & (1 << 1)) != 0 {
        row[at + 1] = post.light_leak;
    }
    if (fijo_mask & (1 << 2)) != 0 {
        row[at + 2] = post.ghost;
    }
    if (fijo_mask & (1 << 3)) != 0 {
        row[at + 3] = post.kaleido;
    }
    if (fijo_mask & (1 << 4)) != 0 {
        row[at + 4] = post.crt;
    }
    if (fijo_mask & (1 << 5)) != 0 {
        row[at + 5] = post.glitch;
    }
    if (fijo_mask & (1 << 6)) != 0 {
        row[at + 6] = post.posterize;
    }
    if (fijo_mask & (1 << 7)) != 0 {
        row[at + 7] = post.zoom;
    }
    if (fijo_mask & (1 << 8)) != 0 {
        row[at + 8] = post.fire;
    }
    if (fijo_mask & (1 << 9)) != 0 {
        row[at + 9] = post.lorenz;
    }
    if (fijo_mask & (1 << 10)) != 0 {
        row[at + 10] = post.vignette;
    }
    if (fijo_mask & (1 << 11)) != 0 {
        row[at + 11] = post.blur;
    }
    if (fijo_mask & (1 << 12)) != 0 {
        row[at + 12] = post.sharpen;
    }
    if (fijo_mask & (1 << 13)) != 0 {
        row[at + 13] = post.grain;
    }

    // Color slot: bit 14 controls the leak_color. It trails the *whole*
    // effects block, the advanced floats included (`EFFECTS_FLOATS.len()`,
    // not the common-effect count 14), so widening the row did not make this
    // write into `grade_temperature`.
    let leak_at = at + metal_schema::EFFECTS_FLOATS.len();
    if (fijo_mask & (1 << 14)) != 0 {
        for (dst, src) in row[leak_at..leak_at + 12].iter_mut().zip(post.leak_color.iter()) {
            *dst = *src;
        }
    }
}

/// Routes a list of layers onto one device — what `POST /layers {op:"apply"}`
/// and the corpus fixtures both use to stamp a stack onto a screen. The
/// device id replaces every layer's `outputs`, so the pool's routing is the
/// one thing this deliberately overwrites: applying a collection is copying
/// it onto a device, and `Layer::outputs` is where that lands.
pub fn stamped(list: Vec<Layer>, device: &str) -> Vec<Layer> {
    list.into_iter()
        .map(|mut l| {
            l.outputs = vec![device.to_string()];
            l
        })
        .collect()
}

/// A black, opaque frame. What a disabled or empty device shows.
pub fn black(w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
    for p in px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    px
}




/// Two layers sharing model/feed/base (so [`compile_device_plan`] puts
/// them on one `gen.*` node — A5's dedup case, for real this time) plus
/// a third on a different model, different z, different blend, to prove
/// the shared node does not also collapse the layers' own identities.
pub fn fan_out_stack() -> Vec<Layer> {
    let mut back = Layer::new("back", "back", 0);
    back.z = 0;
    back.blend = Blend::Normal;
    back.opacity = 1.0;

    // Same model, same feed, same (absent) base as `back` — the pair
    // `compile_device_plan` must fold onto one node.
    let mut echo = Layer::new("echo", "echo", 0);
    echo.z = 1;
    echo.blend = Blend::Add;
    echo.opacity = 0.4;

    // A different generator, behind both, so draw order still has
    // something to get wrong.
    let mut base_layer = Layer::new("base", "base", 1);
    base_layer.z = -5;
    base_layer.blend = Blend::Normal;

    // Every fixture in this file lands on the preview — `stamped` is the
    // same helper `POST /layers` uses to route a collection's layers
    // onto whichever device it was applied to.
    stamped(vec![back, echo, base_layer], PREVIEW)
}

#[cfg(test)]
mod isolate_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// One graph per collection: saving a collection graph through the
    /// engine's one writer publishes the device graph of every screen its
    /// outputs name, and an edit of a published graph lands in the collection.
    #[test]
    fn saving_a_collection_publishes_its_screens_and_an_edit_writes_back() {
        let root = std::env::temp_dir().join(format!("mvj-engine-collection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut g = metal_graph::Graph::new("col-show", "Show", metal_graph::Domain::Live);
        g.add_node("gen", "gen.turing", [0.0, 0.0]);
        for (i, screen) in ["A", "B"].into_iter().enumerate() {
            let out = g.add_node("out", "output.device", [200.0, 100.0 * i as f32]);
            metal_graph::collection::bind_node(g.node_mut(&out).unwrap(), screen);
        }
        save_graph_at(&root, &g).unwrap();
        let a = load_graph_at(&root, &device_graph_id("A")).expect("A is published");
        assert_eq!(a.source.as_deref(), Some("col-show"));
        assert_eq!(a.nodes.iter().filter(|n| n.kind == "output.device").count(), 1);

        let mut edited = a.clone();
        let fx = edited.add_node("fx", "fx.shake", [100.0, 0.0]);
        save_graph_at(&root, &edited).unwrap();
        let col = load_graph_at(&root, "col-show").unwrap();
        assert!(col.node(&fx).is_some(), "the edit reached the collection");
        let b = load_graph_at(&root, &device_graph_id("B")).unwrap();
        assert!(b.node(&fx).is_some(), "and the other screen");
    }

    /// `Layer::span`: a device showing half of a source gets exactly that
    /// half, and a whole-frame window is no copy at all.
    #[test]
    fn a_span_window_crops_its_part_of_the_frame() {
        // 4x2 frame whose red channel is the column index.
        let px: Vec<u8> = (0..8).flat_map(|i| [(i % 4) as u8, 0, 0, 255]).collect();
        let right = crop_window(&px, 4, 2, &Sector { x: 0.5, y: 0.0, w: 0.5, h: 1.0 }).unwrap();
        assert_eq!((right.1, right.2), (2, 2));
        assert_eq!(right.0.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), [2, 3, 2, 3]);
        assert!(crop_window(&px, 4, 2, &Sector::default()).is_none(), "the whole frame is not a copy");
        assert!(crop_window(&px, 4, 2, &Sector { x: 0.2, y: 0.2, w: 0.0, h: 0.5 }).is_none());
    }

    /// A painter that draws through the CPU stand-in and remembers what each
    /// generator was asked for — the only way to see the canvas-size render
    /// from outside `render_device`, which keeps its intermediate buffers.
    struct SizeRecorder {
        inner: crate::stand::Stand,
        sizes: Vec<(usize, u32, u32)>,
        frames: Vec<(Vec<u8>, u32, u32)>,
    }

    impl SizeRecorder {
        fn new() -> SizeRecorder {
            SizeRecorder { inner: crate::stand::Stand::new(), sizes: Vec::new(), frames: Vec::new() }
        }
    }

    impl Paint for SizeRecorder {
        fn model_count(&self) -> usize {
            self.inner.model_count()
        }
        fn ids(&self) -> Vec<&'static str> {
            self.inner.ids()
        }
        fn names(&self) -> Vec<&'static str> {
            self.inner.names()
        }
        fn schemas(&self) -> Vec<&'static metal_schema::Schema> {
            self.inner.schemas()
        }
        fn solo(
            &mut self,
            index: usize,
            floats: &[f32],
            time: f32,
            w: u32,
            h: u32,
            fondo_alpha: bool,
        ) -> (Vec<u8>, u32, u32) {
            let frame = self.inner.solo(index, floats, time, w, h, fondo_alpha);
            self.sizes.push((index, w, h));
            self.frames.push(frame.clone());
            frame
        }
        fn solo_chain(
            &mut self,
            index: usize,
            floats: &[f32],
            time: f32,
            w: u32,
            h: u32,
            fondo_alpha: bool,
            chain: &[metal_schema::EffectStep],
        ) -> (Vec<u8>, u32, u32) {
            let frame = self.inner.solo_chain(index, floats, time, w, h, fondo_alpha, chain);
            self.sizes.push((index, w, h));
            self.frames.push(frame.clone());
            frame
        }
        fn forge(
            &mut self,
            index: usize,
            key: &str,
            wgsl: &str,
            floats: &[f32],
            time: f32,
            w: u32,
            h: u32,
            fondo_alpha: bool,
            chain: &[metal_schema::EffectStep],
        ) -> (Vec<u8>, u32, u32) {
            let frame = self.inner.forge(index, key, wgsl, floats, time, w, h, fondo_alpha, chain);
            self.sizes.push((index, w, h));
            self.frames.push(frame.clone());
            frame
        }
        fn forge_fx(
            &mut self,
            key: &str,
            wgsl: &str,
            frame: &(Vec<u8>, u32, u32),
            time: f32,
            ingest: &[f32],
        ) -> Option<Vec<u8>> {
            self.inner.forge_fx(key, wgsl, frame, time, ingest)
        }
        fn footage(&self) -> Vec<bool> {
            self.inner.footage()
        }
        fn backdrop(&self) -> Vec<bool> {
            self.inner.backdrop()
        }
        fn reel(&mut self, index: usize) -> Option<&mut dyn metal_reel::Reel> {
            self.inner.reel(index)
        }
        fn real(&self) -> bool {
            self.inner.real()
        }
        fn describe(&self) -> String {
            self.inner.describe()
        }
    }

    /// The device frame a window of `canvas` becomes: crop the window out and
    /// composite it into the device exactly as `render_device` does.
    fn composed_from_crop(
        layer: &Layer,
        canvas: &[u8],
        cw: u32,
        ch: u32,
        win: &Sector,
        w: u32,
        h: u32,
    ) -> Vec<u8> {
        let crop = crop_window(canvas, cw, ch, win).expect("a real window crops");
        let tile = compose::Tile::of(layer, &crop.0, crop.1, crop.2, layer.sector, None, &[]);
        compose::compose(w, h, Composition::Overlap, &[tile])
    }

    /// P3 item 3: a source spanning two devices renders **once** at the
    /// covering canvas size, and each device crops its own window out of that
    /// one render — not out of a device-sized render it then upscales.
    #[test]
    fn a_spanned_source_renders_once_at_canvas_size() {
        let a = format!("span-a-{}", std::process::id());
        let b = format!("span-b-{}", std::process::id());
        let mut paint = SizeRecorder::new();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(&paint);
        state.paused_at = Some(3.0);
        let (w, h) = (64u32, 32u32);

        let left = Sector { x: 0.0, y: 0.0, w: 0.5, h: 1.0 };
        let right = Sector { x: 0.5, y: 0.0, w: 0.5, h: 1.0 };
        // Model 1 (vertical bars) is not left/right symmetric, so the two
        // windows really are two different pictures.
        let mut layer = Layer::new("spanned", "spanned", 1);
        layer.outputs = vec![a.clone(), b.clone()];
        layer.span.insert(a.clone(), left);
        layer.span.insert(b.clone(), right);
        state.layers = vec![layer];
        let own = layers::Device { mode: layers::DeviceMode::Own, ..layers::Device::default() };
        state.sources.insert(a.clone(), own.clone());
        state.sources.insert(b.clone(), own);

        assert_eq!(
            span_canvas_size(&state.layers[0].span, w, h),
            (2 * w, h),
            "the covering canvas renders the half-window at device resolution"
        );

        paint.sizes.clear();
        let (frame_a, fw, fh) = render_device(&mut paint, &mut sim, &mut state, &a, w, h);
        assert_eq!((fw, fh), (w, h), "the device frame stays device-sized");
        assert_eq!(
            paint.sizes,
            vec![(1, 2 * w, h)],
            "the generator is painted once, at the canvas size"
        );
        let (canvas_a, cw, ch) = paint.frames.last().unwrap().clone();
        let expect_a = composed_from_crop(&state.layers[0], &canvas_a, cw, ch, &left, w, h);
        assert_eq!(frame_a, expect_a, "device a shows its window of the one canvas");

        paint.sizes.clear();
        let (frame_b, _, _) = render_device(&mut paint, &mut sim, &mut state, &b, w, h);
        assert_eq!(paint.sizes, vec![(1, 2 * w, h)], "device b renders the same canvas once");
        let (canvas_b, cw_b, ch_b) = paint.frames.last().unwrap().clone();
        let expect_b = composed_from_crop(&state.layers[0], &canvas_b, cw_b, ch_b, &right, w, h);
        assert_eq!(frame_b, expect_b, "device b shows its own window of the canvas");
        assert_ne!(frame_a, frame_b, "the two devices crop different windows");
    }

    use crate::layers::{Blend, Collection, Composition};
    use crate::live::Input;
    use crate::output::Output;
    use crate::paint;
    use crate::{overlay, sequence};

    /// O.6's route, the engine half: a script's row on a `mask` node's
    /// `control` becomes that device mask's contour for the frame — and
    /// only for the frame's copy. The row is the polygon (`x, y, …`); too
    /// short a row, an unnamed mask, a bypassed node or a `compose`d mask
    /// all leave the contour exactly as drawn.
    #[test]
    fn a_script_row_on_a_mask_nodes_control_is_that_masks_contour() {
        use metal_graph::{Domain, Endpoint, Graph};
        let mut g = Graph::new("g", "t", Domain::Live);
        let s = g.add_node("s", "script.py", [0.0, 0.0]);
        let m = g.add_node("m", "mask", [1.0, 0.0]);
        g.node_mut(&m).unwrap().config = metal_graph::Config::from_json_str(r#"{"mask":"side"}"#).unwrap();
        g.add_edge(Endpoint::new(&s, "control"), Endpoint::new(&m, "control"));
        // A second mask node with no script behind it, naming another mask.
        let other = g.add_node("o", "mask", [1.0, 1.0]);
        g.node_mut(&other).unwrap().config = metal_graph::Config::from_json_str(r#"{"mask":"drawn"}"#).unwrap();

        let drawn = vec![[0.1, 0.1], [0.9, 0.1], [0.5, 0.9]];
        let mut masks = vec![
            Mask { id: "side".into(), name: "side".into(), points: drawn.clone(), ..Mask::default() },
            Mask { id: "drawn".into(), name: "drawn".into(), points: drawn.clone(), ..Mask::default() },
        ];

        // The shot's own answer: the person is at the right, width 0.64 —
        // the script wrote the rectangle from x = 0.36 to the edge.
        let mut rows = std::collections::HashMap::new();
        rows.insert(s.clone(), vec![0.36, 0.0, 1.0, 0.0, 1.0, 1.0, 0.36, 1.0]);
        apply_script_masks(&g, &rows, &mut masks);
        assert_eq!(masks[0].points, vec![[0.36, 0.0], [1.0, 0.0], [1.0, 1.0], [0.36, 1.0]]);
        assert!(masks[0].closed());
        assert_eq!(masks[1].points, drawn, "a mask no script feeds keeps its drawn contour");

        // Under three points: not a shape yet, contour left alone.
        rows.insert(s.clone(), vec![0.5, 0.5]);
        let before = masks[0].points.clone();
        apply_script_masks(&g, &rows, &mut masks);
        assert_eq!(masks[0].points, before);

        // No row this frame (the script errored): contour left alone.
        rows.clear();
        apply_script_masks(&g, &rows, &mut masks);
        assert_eq!(masks[0].points, before);
    }

    /// R22.5b: the boot migration folds a mode node into the output's config —
    /// a legacy `stack.mode`, or a `compose.*` mixer whose members are
    /// re-cabled straight onto `output.device.image`. A graph that already
    /// declares the mode on its output is left exactly as it is.
    /// A generator wired into the output decides what the device draws, even
    /// when the output still carries a stored `automix` (the boot migration
    /// stamps the device's default there). No router means no automix.
    #[test]
    fn a_wired_generator_without_a_router_draws_the_gen_not_automix() {
        for stamp in [Some(r#"{"kind":"automix"}"#), None] {
            let paint = paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.preview.mode = layers::DeviceMode::Automix;
            let manifest_ids = paint.ids();
            let device = format!("wired-no-router-{}", stamp.is_some());
            let mut g = metal_graph::Graph::new(&device_graph_id(&device), "c", metal_graph::Domain::Live);
            let gen = g.add_node("gen", &format!("gen.{}", manifest_ids[0]), [0.0, 0.0]);
            let out = g.add_node("out", "output.device", [1.0, 0.0]);
            if let Some(cfg) = stamp {
                g.node_mut(&out).unwrap().config = metal_graph::Config::from_json_str(
                    &format!(r#"{{"mode":{}}}"#, cfg),
                )
                .unwrap();
            }
            g.add_edge(
                metal_graph::Endpoint::new(gen.clone(), "image"),
                metal_graph::Endpoint::new(out, "image"),
            );
            save_graph(&g).unwrap();
            let air = state.airing(&mut sim, state.time());
            let (list, _graph, _plan, _nodes, _remote, mode) = compile_device_plan(
                &state.preview,
                true,
                &device,
                &state.layers,
                &air,
                state.router.group(),
                0.0,
                &manifest_ids,
                paint.model_count(),
                state.overlays.on(&device, state.time()),
                &state.collections,
                None,
                "laptop",
            )
            .expect("a wired generator must draw");
            assert_eq!(mode, layers::DeviceMode::Own, "stamp {stamp:?}");
            assert_eq!(list.len(), 1, "only the wired generator, stamp {stamp:?}");
            assert_eq!(list[0].id, gen);
        }
    }

    /// A `router` wired into the output is the automix switch, whatever the
    /// output's own config says.
    #[test]
    fn a_wired_router_still_automixes() {
        let paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let manifest_ids = paint.ids();
        let device = "router-wired";
        let mut g = metal_graph::Graph::new(&device_graph_id(device), "r", metal_graph::Domain::Live);
        let gen = g.add_node("gen", &format!("gen.{}", manifest_ids[0]), [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        let router = g.add_node("router", ROUTER_KIND, [400.0, 0.0]);
        g.add_edge(metal_graph::Endpoint::new(router, "image"), metal_graph::Endpoint::new(out.clone(), "image"));
        save_graph(&g).unwrap();
        assert_eq!(resolve_authored_mode(&g), Some(layers::DeviceMode::Automix));
        let air = state.airing(&mut sim, state.time());
        let (list, _graph, _plan, _nodes, _remote, mode) = compile_device_plan(
            &state.preview,
            true,
            device,
            &state.layers,
            &air,
            state.router.group(),
            0.0,
            &manifest_ids,
            paint.model_count(),
            state.overlays.on(device, state.time()),
            &state.collections,
            None,
            "laptop",
        )
        .expect("an automix router must draw");
        assert_eq!(mode, layers::DeviceMode::Automix);
        assert!(list.iter().all(|l| l.id != gen), "the automix list is the group, not the canvas");
        let _ = out;
    }

    /// The legacy `automix` output mode migrates to an explicit router when no
    /// generator is wired; a wired canvas keeps itself and loses the automix.
    #[test]
    fn a_legacy_automix_output_migrates_to_a_router_or_to_own() {
        use metal_graph::{Domain, Endpoint, Graph};
        let dir = std::env::temp_dir().join(format!("fw-router-mig-{}", std::process::id()));
        let _ = metal_vfs::create_dir_all(&dir);
        let stamp = |g: &mut Graph, out: &str| {
            g.node_mut(out).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"mode":{"kind":"automix"}}"#).unwrap();
        };

        // Empty device: the automix becomes a router wired into the output.
        let device = "legacy-empty".to_string();
        let mut g = Graph::new(device_graph_id(&device), &device, Domain::Live);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        stamp(&mut g, &out);
        save_graph_at(&dir, &g).unwrap();
        migrate_device_modes_to_graphs_at(&dir, &[(device.clone(), layers::DeviceMode::Automix)]);
        let g = load_graph_at(&dir, &device_graph_id(&device)).unwrap();
        let router = g.nodes.iter().find(|n| n.kind == ROUTER_KIND).expect("a router is wired in");
        assert!(
            g.edges.iter().any(|e| e.from.node == router.id && e.to.node == out && e.to.port == "image"),
            "the router feeds the output's image port"
        );
        assert_eq!(resolve_authored_mode(&g), Some(layers::DeviceMode::Automix));

        // Wired canvas: no router, and the stamped automix is rewritten to own.
        let device = "legacy-wired".to_string();
        let mut g = Graph::new(device_graph_id(&device), &device, Domain::Live);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        stamp(&mut g, &out);
        save_graph_at(&dir, &g).unwrap();
        migrate_device_modes_to_graphs_at(&dir, &[(device.clone(), layers::DeviceMode::Automix)]);
        let g = load_graph_at(&dir, &device_graph_id(&device)).unwrap();
        assert!(!g.nodes.iter().any(|n| n.kind == ROUTER_KIND), "no router for a wired canvas");
        assert_eq!(output_device_mode(&g), Some(layers::DeviceMode::Own));
        assert_eq!(resolve_authored_mode(&g), Some(layers::DeviceMode::Own));

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The node preview of a generator and the output's frame are the same
    /// picture at one instant: same clock, same row, same post.
    #[test]
    fn the_node_preview_and_the_output_frame_agree_at_one_instant() {
        let mut paint = crate::paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);
        let manifest_ids = paint.ids();
        let device = format!("agree-{}", std::process::id());
        let mut g = metal_graph::Graph::new(&device_graph_id(&device), "a", metal_graph::Domain::Live);
        let gen = g.add_node("gen", &format!("gen.{}", manifest_ids[0]), [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        g.add_edge(metal_graph::Endpoint::new(gen.clone(), "image"), metal_graph::Endpoint::new(out, "image"));
        save_graph(&g).unwrap();
        state.sources.insert(
            device.clone(),
            layers::Device { mode: layers::DeviceMode::Own, ..layers::Device::default() },
        );
        let (w, h) = (64u32, 36u32);
        let (frame, fw, fh) = render_device(&mut *paint, &mut sim, &mut state, &device, w, h);
        assert_eq!((fw, fh), (w, h));
        let node = render_node_picture(&mut *paint, &mut sim, &mut state, &device, &gen, w, h, false)
            .expect("the generator has a picture");
        let NodePicture::Cpu(preview, pw, ph) = node else {
            panic!("a CPU picture was asked for");
        };
        assert_eq!((pw, ph), (w, h));
        assert_eq!(preview, frame, "focusing the generator and the output show the same frame");
    }

    #[test]
    fn the_mode_migration_folds_every_shape_into_the_output_config() {
        use metal_graph::{Domain, Endpoint, Graph};
        let dir = std::env::temp_dir().join(format!("fw-mode-mig-{}", std::process::id()));
        let _ = metal_vfs::create_dir_all(&dir);

        // 1. Legacy `stack.mode`.
        let device = "legacy".to_string();
        let mut g = Graph::new(device_graph_id(&device), &device, Domain::Live);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        let stack = g.add_node("m", "stack.mode", [0.0, 0.0]);
        g.node_mut(&stack).unwrap().config =
            metal_graph::Config::from_json_str(r#"{"kind":"automix"}"#).unwrap();
        save_graph_at(&dir, &g).unwrap();
        migrate_device_modes_to_graphs_at(&dir, &[(device.clone(), layers::DeviceMode::Automix)]);
        let g = load_graph_at(&dir, &device_graph_id(&device)).unwrap();
        assert!(!g.nodes.iter().any(|n| n.kind == "stack.mode"), "the legacy node is retired");
        assert_eq!(
            output_device_mode(&g),
            Some(layers::DeviceMode::Automix),
            "the mode lands on the output's config"
        );

        // 2. A `compose.layers` mixer with two members: fan-in onto the output.
        let device = "mixer".to_string();
        let mut g = Graph::new(device_graph_id(&device), &device, Domain::Live);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        let comp = g.add_node("comp", "compose.layers", [-200.0, 0.0]);
        let a = g.add_node("a", "gen.turing", [0.0, 0.0]);
        let b = g.add_node("b", "gen.turing", [0.0, 100.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&comp, "image"));
        g.add_edge(Endpoint::new(&b, "image"), Endpoint::new(&comp, "image"));
        g.add_edge(Endpoint::new(&comp, "image"), Endpoint::new(&out, "image"));
        save_graph_at(&dir, &g).unwrap();
        migrate_device_modes_to_graphs_at(&dir, &[(device.clone(), layers::DeviceMode::Own)]);
        let g = load_graph_at(&dir, &device_graph_id(&device)).unwrap();
        assert!(!g.nodes.iter().any(|n| metal_graph::is_compose_kind(&n.kind)));
        assert_eq!(output_device_mode(&g), Some(layers::DeviceMode::Own));
        let into_out = g
            .edges
            .iter()
            .filter(|e| e.to.node == out && e.to.port == "image")
            .count();
        assert_eq!(into_out, 2, "both members fan in straight onto the output");

        // 3. Already migrated: byte-for-byte untouched.
        let path = graph_path_at(&dir, &device_graph_id(&device));
        let before = metal_vfs::read(&path).unwrap();
        migrate_device_modes_to_graphs_at(&dir, &[(device.clone(), layers::DeviceMode::Automix)]);
        assert_eq!(before, metal_vfs::read(&path).unwrap(), "a migrated graph is not rewritten");

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// R22.6 decision 2: a `mask` node between a branch and the device cuts
    /// *that* branch and only it. Two generators, only one through the mask;
    /// the walk reports the cut on the masked branch and `None` on the other,
    /// and the mask node itself is never mistaken for a generator.
    #[test]
    fn a_mask_node_cuts_only_the_branch_behind_it() {
        use metal_graph::{Domain, Endpoint, Graph};
        let mut g = Graph::new("g", "t", Domain::Live);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        let masked = g.add_node("masked", "gen.turing", [0.0, 0.0]);
        let bare = g.add_node("bare", "gen.turing", [0.0, 100.0]);
        let mask = g.add_node("mask", "mask", [200.0, 0.0]);
        g.node_mut(&mask).unwrap().config =
            metal_graph::Config::from_json_str(r#"{"device":"wall","mask":"side"}"#).unwrap();
        // The masked branch: gen → mask → out; the bare one straight in.
        g.add_edge(Endpoint::new(&masked, "image"), Endpoint::new(&mask, "image"));
        g.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&bare, "image"), Endpoint::new(&out, "image"));

        let gens = authored_generators_with_masks(&g);
        assert_eq!(gens.len(), 2, "the mask node is not a generator of its own");
        let cut_of = |id: &str| gens.iter().find(|(n, _)| n == id).unwrap().1.clone();
        assert_eq!(cut_of(&masked), Some("side".to_string()));
        assert_eq!(cut_of(&bare), None, "the branch with no mask node stays uncut");

        // The ids-only walk still returns both, in edge order.
        assert_eq!(authored_generators(&g).len(), 2);

        // `graph_has_mask_node` agrees a live, named mask node is a cut…
        save_graph_at(&output::data_root(), &g).unwrap();
        let device = format!("mask-node-{}", std::process::id());
        let mut g2 = g.clone();
        g2.id = device_graph_id(&device);
        save_graph_at(&output::data_root(), &g2).unwrap();
        assert!(graph_has_mask_node(&output::data_root(), &device));
        // …and a bypassed one does not.
        let mask_id = g2.nodes.iter().find(|n| n.kind == "mask").unwrap().id.clone();
        g2.node_mut(&mask_id).unwrap().bypass = true;
        save_graph_at(&output::data_root(), &g2).unwrap();
        assert!(!graph_has_mask_node(&output::data_root(), &device));
        let _ = metal_vfs::remove_file(graph_path_at(
            &output::data_root(),
            &device_graph_id(&device),
        ));
    }

    /// L22.12b / D7 end to end: a device whose graph carries a live `mask`
    /// node never reaches `apply_masks` (R22.6 decision 3), so `render_device`
    /// paints its outlines — and a generator wired only to that node's
    /// `stroke` port changes what the band samples, with the flat colour as
    /// the fallback when nothing is wired.
    ///
    /// The two frames are rendered through the real `render_device` from the
    /// same state, the only difference the `stroke` edge: the material must be
    /// what moves the pixels, not some other change in the graph.
    #[test]
    fn a_stroke_material_wired_to_a_mask_node_changes_the_rim_it_paints() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("stroke-material-{}", std::process::id());
        let root = output::data_root();
        let mut paint = crate::paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        // The device, one contour on it, and a flat green stroke: with no
        // material wired this is exactly what the band paints.
        let mut out = Output::new(&device, "stroke material", 64, 64);
        out.masks.push(Mask {
            id: "side".into(),
            name: "side".into(),
            points: vec![[0.15, 0.15], [0.85, 0.15], [0.85, 0.85], [0.15, 0.85]],
            stroke: Some(output::MaskStroke {
                width: 8.0,
                color: Some([0.0, 1.0, 0.0, 1.0]),
                source: None,
            }),
            ..Mask::default()
        });
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut masked = Layer::new("masked", "masked", 0);
        masked.outputs = vec![device.clone()];
        masked.mask = Some("side".into());
        state.layers = vec![masked];

        let ids = paint.ids();
        let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
        let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
        // `compose.layers` is what makes the authored graph *be* the
        // composition (`graph_composed_layers`), rather than the pool list
        // being synthesized from the layers and the mask node dropped.
        graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
        let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
        graph.node_mut(&gen).unwrap().config =
            metal_graph::Config::from_json_str(&json!({ "source_id": "masked" }).to_string())
                .unwrap();
        let mat = graph.add_node("mat", &format!("gen.{}", ids[1]), [0.0, 200.0]);
        let mask = graph.add_node("mask", "mask", [200.0, 0.0]);
        graph.node_mut(&mask).unwrap().config = metal_graph::Config::from_json_str(
            &json!({ "device": device, "mask": "side", "stroke_width": 8.0 }).to_string(),
        )
        .unwrap();
        graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "image"));
        graph.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&out_node, "image"));
        graph.add_edge(Endpoint::new(&mat, "image"), Endpoint::new(&mask, "stroke"));
        save_graph_at(&root, &graph).unwrap();

        let (with_material, _, _) =
            render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        // Same graph, minus the material edge: the band falls back to the flat
        // green — the pre-lot behaviour, unchanged.
        let stroke_edges: Vec<String> = graph
            .edges
            .iter()
            .filter(|e| e.to.port == "stroke")
            .map(|e| e.id.clone())
            .collect();
        for id in stroke_edges {
            graph.remove_edge(&id);
        }
        save_graph_at(&root, &graph).unwrap();
        let (without, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        assert!(
            with_material.iter().any(|b| *b != 0),
            "a stroked mask on a mask-node device must paint something"
        );
        assert_ne!(
            with_material, without,
            "wiring a material to `stroke` must change the band the frame paints"
        );

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
    }

    /// Parity P-E: a source with no model row — `text.source`, `gen.ramp`
    /// — composes as a tile and animates with the same in-transition a
    /// `gen.*` gets: half-way through a 1 s fade it is dimmer than steady,
    /// and not black.
    #[test]
    fn non_model_sources_compose_and_fade_in() {
        use metal_graph::{Domain, Endpoint, Graph};
        for kind in [metal_graph::TEXT_SOURCE_KIND, metal_graph::RAMP_KIND] {
            let device = format!("pe-src-{}-{}", kind.replace('.', "-"), std::process::id());
            let root = output::data_root();
            let mut paint = paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.outputs.list.push(Output::new(&device, "pe src", 64, 64));
            state.sources.insert(device.clone(), layers::Device::default());

            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let src = graph.add_node("src", kind, [0.0, 0.0]);
            let cfg = json!({
                "content": "HOLA", "size": 0.5, "margin": 0.0,
                "transition_in": {"kind": "fade", "duration": 1.0, "unit": "seconds", "ease": "linear"}
            });
            graph.node_mut(&src).unwrap().config = metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
            graph.node_mut(&src).unwrap().bypass = true;
            graph.add_edge(Endpoint::new(&src, "image"), Endpoint::new(&out_node, "image"));
            let on = [crate::switch::SwitchEntry::node(src.clone(), true)];
            state.transitions.stamp(&graph, &on, 10.0);
            crate::switch::apply(&mut graph, &on);
            save_graph_at(&root, &graph).unwrap();
            assert!(
                graph_composed_layers(&graph, &state.layers, &paint.ids()).is_some_and(|l| l.len() == 1),
                "{kind} composes as a tile"
            );

            let lit = |f: &[u8]| f.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum::<u64>();
            state.paused_at = Some(10.5);
            let (mid, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
            state.paused_at = Some(12.0);
            let (steady, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
            let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
            let (m, s) = (lit(&mid), lit(&steady));
            assert!(s > 0, "{kind}: the steady frame shows the source");
            assert!(m > 0 && (m as f64) < s as f64 * 0.8, "{kind}: mid-fade {m} vs steady {s}");
        }
    }

    /// Task #8 end to end: a `mask` node carrying a 3D binding warps the layer
    /// it is wired to through the output's homography for that wall, and — when
    /// it names a contour — cuts the frame to the contour's projection. The
    /// whole-wall binding isolates the warp (no contour, no cut): the frame
    /// must still differ from the same graph with no 3D node, which can only be
    /// the per-(output, wall) homography applied in `compose`.
    #[test]
    fn a_wall_binding_warps_its_layer_through_the_scene_homography() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("wall-3d-{}", std::process::id());
        let root = output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let out = Output::new(&device, "wall 3d", 64, 64);
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut layer = Layer::new("wall-layer", "wall layer", 0);
        layer.outputs = vec![device.clone()];
        state.layers = vec![layer];

        // A wall the output sees from off to the side — a perspective view, so
        // the homography is not the identity — and one contour drawn on it.
        // `save` lands in the per-process scratch scene under `data_root`
        // (rule 8), so no real `data/` is touched.
        let wall = escena::Wall {
            id: "front".into(),
            name: "front".into(),
            origin: [-1.0, 1.0, 0.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, -2.0, 0.0],
            ..Default::default()
        };
        let scene = escena::Escena {
            walls: vec![wall.clone()],
            views: vec![escena::View {
                output: device.clone(),
                eye: [0.4, 0.25, 1.0],
                target: [0.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0],
                fov: 80.0,
                aspect: 1.0,
            }],
            masks: vec![escena::WallMask {
                id: "patch".into(),
                name: "patch".into(),
                wall: "front".into(),
                points: vec![[0.2, 0.2], [0.6, 0.2], [0.6, 0.6], [0.2, 0.6]],
                parent: None,
                alpha: 1.0,
            }],
        };
        scene.save(&root).expect("save the scratch scene");
        let h = scene
            .homography(&device, &wall)
            .expect("the view is a map from the wall to this output");
        assert!(
            h.map([0.15, 0.15])
                .is_some_and(|p| (p[0] - 0.15).abs() + (p[1] - 0.15).abs() > 0.01),
            "the test scene must warp, not be the identity"
        );

        let ids = paint.ids();
        // `bind` is `None` for the flat graph, `Some(None)` for whole-wall and
        // `Some(Some("patch"))` for the contour-bound one.
        let build = |bind: Option<Option<&str>>| {
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            // `compose.layers` is what makes the authored graph *be* the
            // composition, so the mask node is kept and binds the layer.
            graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "wall-layer" }).to_string())
                    .unwrap();
            match bind {
                None => {
                    graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out_node, "image"));
                }
                Some(contour) => {
                    let mask = graph.add_node("mask", "mask", [200.0, 0.0]);
                    let cfg = match contour {
                        Some(c) => json!({ "wall": "front", "contour": c }),
                        None => json!({ "wall": "front" }),
                    };
                    graph.node_mut(&mask).unwrap().config =
                        metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
                    graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "image"));
                    graph.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&out_node, "image"));
                }
            }
            graph
        };

        save_graph_at(&root, &build(None)).unwrap();
        let (flat, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        save_graph_at(&root, &build(Some(None))).unwrap();
        assert!(
            !graph_has_mask_node(&root, &device),
            "a whole-wall binding cuts nothing, so the whole-frame pass still stands"
        );
        let (whole_wall, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        save_graph_at(&root, &build(Some(Some("patch")))).unwrap();
        assert!(
            graph_has_mask_node(&root, &device),
            "a named wall contour is a graph cut the caller must not repeat"
        );
        let (cut, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        assert_ne!(
            flat, whole_wall,
            "a whole-wall binding must warp its layer even though it cuts nothing"
        );
        assert!(
            whole_wall.iter().any(|b| *b != 0),
            "the warped layer must still draw something"
        );
        assert_ne!(
            whole_wall, cut,
            "naming a contour must add the projected cut on top of the warp"
        );
        assert!(
            cut.iter().step_by(4).any(|b| *b == 0),
            "the projected contour must black part of the frame"
        );

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        let _ = metal_vfs::remove_file(escena::path(&root));
    }

    /// P2: a source pinned to a `venue.surface` lands where the projector's
    /// hand-dragged `pins` put that surface — inside them, not across the
    /// whole frame. The pins are a quad well inside the device, so a lit pixel
    /// outside it is the warp not happening.
    #[test]
    fn a_layer_through_a_surface_lands_inside_its_pins() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("surface-pins-{}", std::process::id());
        let root = output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let out = Output::new(&device, "surface", 64, 64);
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut layer = Layer::new("surface-layer", "surface layer", 0);
        layer.outputs = vec![device.clone()];
        state.layers = vec![layer];

        // A venue whose projector covers the wall with a pin quad inside the
        // frame. Written to the per-process scratch (rule 8), never `data/`.
        let toml = format!(
            r#"
[venue]
id = "surf-test"
name = "Surface Test"
setting = "indoor"

[[surface]]
id = "front"
name = "Front"
origin = [-1.0, 1.0, 0.0]
u = [2.0, 0.0, 0.0]
v = [0.0, -2.0, 0.0]

[[object]]
id = "table"
name = "Table"
kind = "table"
at = [0.0, 0.0, 2.0]
size = [1.0, 0.75, 1.0]

[[projector]]
id = "proj"
name = "Proj"
output = "{device}"
on = "table"
yaw = 0.0
pitch = -10.0
throw_ratio = 1.5

[projector.pins]
front = [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]]
"#
        );
        metal_vfs::create_dir_all(crate::venue::dir(&root)).unwrap();
        metal_vfs::write(crate::venue::dir(&root).join("surf-test.toml"), &toml).unwrap();

        let ids = paint.ids();
        let build = |surface: bool| {
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "surface-layer" }).to_string())
                    .unwrap();
            if surface {
                let s = graph.add_node("surface", metal_graph::VENUE_SURFACE_KIND, [200.0, 0.0]);
                graph.node_mut(&s).unwrap().config = metal_graph::Config::from_json_str(
                    &json!({ "venue": "surf-test", "surface": "front" }).to_string(),
                )
                .unwrap();
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&s, "image"));
                graph.add_edge(Endpoint::new(&s, "image"), Endpoint::new(&out_node, "image"));
            } else {
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out_node, "image"));
            }
            graph
        };

        save_graph_at(&root, &build(false)).unwrap();
        let (flat, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        save_graph_at(&root, &build(true)).unwrap();
        let (warped, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        assert_ne!(flat, warped, "a venue.surface node must warp its layer");
        let lit = |px: &[u8], x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            // Colour only: the frame is opaque black, so alpha is 255
            // everywhere and would read every pixel as lit.
            px[i..i + 3].iter().any(|b| *b != 0)
        };
        // Every lit pixel of the warped frame sits inside the pin quad (with a
        // couple of pixels of bilinear margin), and the flat frame lights the
        // corners the pins leave dark.
        for y in 0..64 {
            for x in 0..64 {
                if lit(&warped, x, y) {
                    assert!(
                        (10..=54).contains(&x) && (10..=54).contains(&y),
                        "warped pixel ({x},{y}) is outside the pins"
                    );
                }
            }
        }
        assert!(lit(&warped, 32, 32), "the pinned centre must be lit");
        assert!(!lit(&warped, 2, 2), "the corner outside the pins stays black");
        // The stand-in lights every pixel, so its flat frame proves the
        // corners are reachable; a real model's picture (geoterrain at its
        // defaults lights ~6% of a 64² frame, all central) may not reach
        // them, and `assert_ne!` above already proves the surface warped it.
        if !paint.real() {
            assert!(lit(&flat, 2, 2), "with no surface node the whole frame is drawn");
        }

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        let _ = metal_vfs::remove_file(crate::venue::dir(&root).join("surf-test.toml"));
    }

    /// P2.4: a `venue.surface` node whose venue is readable but whose surface
    /// has neither pins nor a fit to derive one from is no warp — the layer
    /// draws nothing, not the projector's whole frame. The old whole-wall
    /// fallback would have lit the frame edge to edge.
    #[test]
    fn no_fit_and_no_pins_is_no_warp_not_a_whole_wall() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("surface-nofit-{}", std::process::id());
        let root = output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let out = Output::new(&device, "surface", 64, 64);
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut layer = Layer::new("surface-layer", "surface layer", 0);
        layer.outputs = vec![device.clone()];
        state.layers = vec![layer];

        // A readable venue whose projector covers the wall but carries no pin
        // set for it. Written to the per-process scratch (rule 8), never `data/`.
        let toml = format!(
            r#"
[venue]
id = "nofit-test"
name = "No Fit Test"
setting = "indoor"

[[surface]]
id = "front"
name = "Front"
origin = [-1.0, 1.0, 0.0]
u = [2.0, 0.0, 0.0]
v = [0.0, -2.0, 0.0]

[[object]]
id = "table"
name = "Table"
kind = "table"
at = [0.0, 0.0, 2.0]
size = [1.0, 0.75, 1.0]

[[projector]]
id = "proj"
name = "Proj"
output = "{device}"
on = "table"
yaw = 0.0
pitch = -10.0
throw_ratio = 1.5
"#
        );
        metal_vfs::create_dir_all(crate::venue::dir(&root)).unwrap();
        metal_vfs::write(crate::venue::dir(&root).join("nofit-test.toml"), &toml).unwrap();

        let ids = paint.ids();
        let build = |surface: bool| {
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "surface-layer" }).to_string())
                    .unwrap();
            if surface {
                let s = graph.add_node("surface", metal_graph::VENUE_SURFACE_KIND, [200.0, 0.0]);
                graph.node_mut(&s).unwrap().config = metal_graph::Config::from_json_str(
                    &json!({ "venue": "nofit-test", "surface": "front" }).to_string(),
                )
                .unwrap();
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&s, "image"));
                graph.add_edge(Endpoint::new(&s, "image"), Endpoint::new(&out_node, "image"));
            } else {
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out_node, "image"));
            }
            graph
        };

        save_graph_at(&root, &build(false)).unwrap();
        let (flat, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        assert!(flat.iter().any(|b| *b != 0), "the flat frame must draw something");

        save_graph_at(&root, &build(true)).unwrap();
        let (unmapped, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        let lit = |px: &[u8], x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            px[i..i + 3].iter().any(|b| *b != 0)
        };
        assert_ne!(flat, unmapped, "a surface with nothing to map must not draw the whole wall");
        assert!(
            !(0u32..64).any(|y| (0u32..64).any(|x| lit(&unmapped, x, y))),
            "no pin, no fit: the surface layer draws nothing"
        );

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        let _ = metal_vfs::remove_file(crate::venue::dir(&root).join("nofit-test.toml"));
    }

    /// P2: a graph with no `venue.surface` node is never warped — and a
    /// `venue.surface` node that names a venue nobody wrote is no warp either
    /// (it reads as `None`), so the frame is byte for byte the flat one.
    #[test]
    fn no_surface_node_means_no_warp() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("surface-none-{}", std::process::id());
        let root = output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let out = Output::new(&device, "surface", 64, 64);
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut layer = Layer::new("surface-layer", "surface layer", 0);
        layer.outputs = vec![device.clone()];
        state.layers = vec![layer];

        let ids = paint.ids();
        let build = |surface: bool| {
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "surface-layer" }).to_string())
                    .unwrap();
            if surface {
                let s = graph.add_node("surface", metal_graph::VENUE_SURFACE_KIND, [200.0, 0.0]);
                graph.node_mut(&s).unwrap().config = metal_graph::Config::from_json_str(
                    &json!({ "venue": "no-such-venue", "surface": "front" }).to_string(),
                )
                .unwrap();
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&s, "image"));
                graph.add_edge(Endpoint::new(&s, "image"), Endpoint::new(&out_node, "image"));
            } else {
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out_node, "image"));
            }
            graph
        };

        save_graph_at(&root, &build(false)).unwrap();
        let (flat, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        assert!(flat.iter().any(|b| *b != 0), "the flat frame must draw something");

        save_graph_at(&root, &build(true)).unwrap();
        let (unread, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        assert_eq!(flat, unread, "a surface node with no venue to read is no warp");

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
    }

    /// P-V: a graph with no `venue.surface` node at all is the flat path, and
    /// the sector work must leave it byte-identical — the frame two flat
    /// sources give is the same as the frame one flat source gives through the
    /// same list, with no cell, clip or render size anywhere in it.
    #[test]
    fn a_graph_with_no_surface_is_the_flat_path() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("surface-flat-{}", std::process::id());
        let root = output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let out = Output::new(&device, "flat", 64, 64);
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut layer = Layer::new("flat-layer", "flat layer", 0);
        layer.outputs = vec![device.clone()];
        state.layers = vec![layer];

        let ids = paint.ids();
        let build = |via_compose: bool| {
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "flat-layer" }).to_string())
                    .unwrap();
            if via_compose {
                let comp = graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&comp, "image"));
                graph.add_edge(Endpoint::new(&comp, "image"), Endpoint::new(&out_node, "image"));
            } else {
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out_node, "image"));
            }
            graph
        };

        save_graph_at(&root, &build(false)).unwrap();
        let (direct, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        save_graph_at(&root, &build(true)).unwrap();
        let (composed, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);

        assert!(direct.iter().any(|b| *b != 0), "the flat frame draws");
        assert_eq!(
            direct, composed,
            "no surface node: routing through a compose mixer changes nothing"
        );
        assert!(
            !authored_generators_with_bindings(&build(false))
                .iter()
                .any(|(_, _, s)| s.is_some()),
            "a flat graph binds no surface"
        );

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
    }

    /// The wire after the device (2026-10-22): `output.device.venue` is a
    /// **terminal view** of the frame the device already composed — never a
    /// second composition (rule 3: "one origin. An output is a view of the
    /// frame"). The proof is the picture: the same graph with the extra
    /// `output.device → venue.surface` cable renders the same pixels, byte
    /// for byte, with the view left dangling — the view reads the frame, it
    /// does not feed anything back into it.
    ///
    /// The view's surface names a venue that does not exist, so a surface
    /// that *were* read as a second place would draw nothing and the frames
    /// would differ. That makes this test bite: a reader that resolves the
    /// back edge as a place would fail it, not pass it silently.
    #[test]
    fn the_device_venue_view_renders_the_same_frame() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("venue-view-{}", std::process::id());
        let root = output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let out = Output::new(&device, "venue-view", 64, 64);
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut layer = Layer::new("venue-view-layer", "venue view layer", 0);
        layer.outputs = vec![device.clone()];
        state.layers = vec![layer];

        let ids = paint.ids();
        let build = |view: bool| {
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            let audio = graph.add_node("audio", "audio.live", [0.0, 160.0]);
            let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "venue-view-layer" }).to_string())
                    .unwrap();
            graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out_node, "image"));
            graph.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out_node, "control"));
            if view {
                let view = graph.add_node("view", metal_graph::VENUE_SURFACE_KIND, [800.0, 160.0]);
                graph.node_mut(&view).unwrap().config = metal_graph::Config::from_json_str(
                    &json!({ "venue": "venue-view-absent", "surface": "front" }).to_string(),
                )
                .unwrap();
                graph.add_edge(Endpoint::new(&out_node, "venue"), Endpoint::new(&view, "image"));
            }
            graph
        };

        save_graph_at(&root, &build(false)).unwrap();
        let (plain, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        assert!(plain.iter().any(|b| *b != 0), "the frame must draw something");

        save_graph_at(&root, &build(true)).unwrap();
        let (viewed, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        assert_eq!(
            plain, viewed,
            "the device's venue output is a view of the frame, not a second composition"
        );

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
    }

    /// P-V: one `venue.surface` node fans in several generators, and deals each
    /// the next cell of its sector grid. Two sources into one two-cell surface
    /// give two layers, each cut to its own half.
    #[test]
    fn a_surface_fans_in_several_sources_into_its_cells() {
        use metal_graph::{Domain, Endpoint, Graph};
        let device = format!("surface-fan-{}", std::process::id());
        let root = output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let out = Output::new(&device, "surface", 64, 64);
        state.outputs.list.push(out);
        state.sources.insert(device.clone(), layers::Device::default());
        let mut a = Layer::new("a", "a", 0);
        a.outputs = vec![device.clone()];
        let mut b = Layer::new("b", "b", 0);
        b.outputs = vec![device.clone()];
        state.layers = vec![a, b];

        // A surface whose pins fill the frame, split into two halves.
        let toml = format!(
            r#"
[venue]
id = "fan-test"
name = "Fan Test"
setting = "indoor"

[[surface]]
id = "front"
name = "Front"
origin = [-1.0, 1.0, 0.0]
u = [2.0, 0.0, 0.0]
v = [0.0, -2.0, 0.0]

[surface.sectors]
source = "projected"
ratio = 1.0

[[object]]
id = "table"
name = "Table"
kind = "table"
at = [0.0, 0.0, 2.0]
size = [1.0, 0.75, 1.0]

[[projector]]
id = "proj"
name = "Proj"
output = "{device}"
on = "table"
yaw = 0.0
pitch = -10.0
throw_ratio = 1.5

[projector.pins]
front = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
"#
        );
        metal_vfs::create_dir_all(crate::venue::dir(&root)).unwrap();
        metal_vfs::write(crate::venue::dir(&root).join("fan-test.toml"), &toml).unwrap();

        let ids = paint.ids();
        let build = |fan: bool| {
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let gen_a = graph.add_node("gen_a", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen_a).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "a" }).to_string())
                    .unwrap();
            let gen_b = graph.add_node("gen_b", &format!("gen.{}", ids[1]), [0.0, 80.0]);
            graph.node_mut(&gen_b).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "b" }).to_string())
                    .unwrap();
            if fan {
                let s = graph.add_node("surface", metal_graph::VENUE_SURFACE_KIND, [200.0, 0.0]);
                graph.node_mut(&s).unwrap().config = metal_graph::Config::from_json_str(
                    &json!({ "venue": "fan-test", "surface": "front" }).to_string(),
                )
                .unwrap();
                graph.add_edge(Endpoint::new(&gen_a, "image"), Endpoint::new(&s, "image"));
                graph.add_edge(Endpoint::new(&gen_b, "image"), Endpoint::new(&s, "image"));
                graph.add_edge(Endpoint::new(&s, "image"), Endpoint::new(&out_node, "image"));
            } else {
                graph.add_edge(Endpoint::new(&gen_a, "image"), Endpoint::new(&out_node, "image"));
                graph.add_edge(Endpoint::new(&gen_b, "image"), Endpoint::new(&out_node, "image"));
            }
            graph
        };

        // The walk gathers both branches through one surface node, in edge
        // order, each tagged with its slot.
        let gens = authored_generators_with_bindings(&build(true));
        assert_eq!(gens.len(), 2, "both sources reach the output through the one surface");
        assert_eq!(gens[0].2.as_ref().and_then(|s| s.sector), Some(0));
        assert_eq!(gens[1].2.as_ref().and_then(|s| s.sector), Some(1));

        save_graph_at(&root, &build(true)).unwrap();
        let (fan, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
        let lit = |px: &[u8], x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            px[i..i + 3].iter().any(|b| *b != 0)
        };
        assert!(lit(&fan, 8, 32), "the first cell (left half) draws");
        assert!(
            (0u32..64).any(|y| lit(&fan, 56, y)),
            "the second cell (right half) draws too"
        );

        let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        let _ = metal_vfs::remove_file(crate::venue::dir(&root).join("fan-test.toml"));
    }

    /// P2: the rectangle a surface-pinned source renders at is the projected
    /// contour's bounding box × 1.5, in device pixels — a patch of wall is not
    /// a device-sized buffer.
    #[test]
    fn render_size_follows_the_projected_bbox() {
        let st = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let xy = [[0.25f32, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]];
        let h = escena::Homography::from_corners(&st, &xy).expect("a valid quad");
        // The quad is 0.5 × 0.5 at the centre; × 1.5 is 0.75 × 0.75, so at
        // 400 × 400 the buffer is 300 × 300.
        assert_eq!(surface_render_size(&h, 400, 400), (300, 300));
        // The composited place is the exact projected bbox, not the inflated
        // render size, so nothing is walked outside the surface's outline.
        assert_eq!(surface_place(&h), Sector { x: 0.25, y: 0.25, w: 0.5, h: 0.5 });
    }

        /// LAY.7b's acceptance, the other half: a `compose.layers` graph with
        /// wired members *is* the composition. The pool's routing is not
        /// consulted, the graph's own order is the draw order, a member the
        /// canvas added (no `source_id`) draws from its own node, and removing a
        /// member changes the frame" — "editing the list in the graph changes
        /// the frame".
        ///
        /// Writes to the repo's own `data/graphs/` under a device id unique to
        /// this process (the same place the app's own device graphs live, which
        /// is gitignored) and removes it again; `output::data_root()` is a
        /// `OnceLock` a test cannot point elsewhere.
        #[test]
        fn a_bypassed_generator_node_is_left_out_of_the_composition() {
            // O.28: the switch flips `Node.bypass`; the composition for that
            // frame must not include the generator, and the pool layer's own
            // `active` must stay as it was (the two are not the same bit).
            let painter = paint::painter();
            let ids = painter.ids();
            let mut graph =
                metal_graph::Graph::new("g", "d", metal_graph::Domain::Live);
            let out = graph.add_node("out", "output.device", [800.0, 0.0]);
            let gen_a = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            let gen_b = graph.add_node("gen", &format!("gen.{}", ids[1]), [0.0, 100.0]);
            for g in [&gen_a, &gen_b] {
                graph.add_edge(
                    metal_graph::Endpoint::new(g, "image"),
                    metal_graph::Endpoint::new(&out, "image"),
                );
            }
            let pool: Vec<Layer> = Vec::new();

            let both = graph_composed_layers(&graph, &pool, &ids).unwrap();
            assert_eq!(both.len(), 2);

            graph.node_mut(&gen_b).unwrap().bypass = true;
            let one = graph_composed_layers(&graph, &pool, &ids).unwrap();
            assert_eq!(one.len(), 1, "a bypassed generator still drew");
            assert_eq!(one[0].id, gen_a);
            assert!(one[0].active, "bypass must not be written into the layer's active");
        }

        #[test]
        fn a_graph_composed_device_takes_its_members_from_the_graph_not_the_pool() {            let device = format!("graph-composed-{}", std::process::id());
            let root = output::data_root();
            let mut paint = crate::paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.paused_at = Some(3.0);
    
            // Two pool layers routed here, one of them bypassed: if the pool were
            // deciding, only `a` would draw.
            let mut a = Layer::new("layer-a", "a", 0);
            a.outputs = vec![device.clone()];
            let mut b = Layer::new("layer-b", "b", 1);
            b.outputs = vec![device.clone()];
            b.active = false;
            state.layers = vec![a, b];
            state.sources.insert(device.clone(), layers::Device::default());
    
            let ids = paint.ids();
            let mut graph =
                metal_graph::Graph::new(&device_graph_id(&device), &device, metal_graph::Domain::Live);
            let out = graph.add_node("out", "output.device", [800.0, 0.0]);
            graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let gen_a = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen_a).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "layer-a" }).to_string())
                    .unwrap();
            // A member added in the canvas: no `source_id`, so it is built from
            // the node itself — full-frame, even though the pool's `b` is
            // bypassed.
            let gen_b = graph.add_node("gen", &format!("gen.{}", ids[1]), [0.0, 100.0]);
            graph.add_edge(
                metal_graph::Endpoint::new(&gen_a, "image"),
                metal_graph::Endpoint::new(&out, "image"),
            );
            graph.add_edge(
                metal_graph::Endpoint::new(&gen_b, "image"),
                metal_graph::Endpoint::new(&out, "image"),
            );
            save_graph_at(&root, &graph).unwrap();
    
            let (both, w, h) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert_eq!((w, h), (64, 36));
            assert!(
                both.iter().any(|b| *b != 0),
                "two wired members must draw something"
            );
    
            // The graph is the list: remove the canvas-added member and the frame
            // changes, with the pool untouched.
            graph.remove_node(&gen_b);
            save_graph_at(&root, &graph).unwrap();
            let (only_a, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert_ne!(both, only_a, "removing a graph member must change the frame");
            assert!(only_a.iter().any(|b| *b != 0));
    
            // No members at all: delegation — the pool decides, and the bypassed
            // `b` is not drawn until the pool says so.
            graph.remove_node(&gen_a);
            save_graph_at(&root, &graph).unwrap();
            let (pool, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert!(pool.iter().any(|b| *b != 0), "the pool's `a` still draws");
            state
                .layers
                .iter_mut()
                .find(|l| l.id == "layer-b")
                .unwrap()
                .active = true;
            let (pool_both, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert_ne!(
                pool, pool_both,
                "delegation must honour the pool's own active/solo rule"
            );
    
            let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        }

        /// `R22.1`: a `gen.*` that exists only as a node of the device's
        /// authored graph — never routed into the pool — still renders its
        /// own picture. The pool is empty for this device, so the only
        /// answer is the graph's own member list. A name that is in neither
        /// the pool nor the graph still answers `None` (the preview paints
        /// nothing rather than the wrong animation).
        #[test]
        fn render_source_finds_a_layer_that_only_exists_as_a_graph_node() {
            let device = format!("source-node-{}", std::process::id());
            let root = output::data_root();
            let mut paint = crate::paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.paused_at = Some(3.0);
            let ids = paint.ids();

            let mut graph = metal_graph::Graph::new(
                &device_graph_id(&device),
                &device,
                metal_graph::Domain::Live,
            );
            let out = graph.add_node("out", "output.device", [800.0, 0.0]);
            let gen = graph.add_node("gen-0", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.add_edge(
                metal_graph::Endpoint::new(&gen, "image"),
                metal_graph::Endpoint::new(&out, "image"),
            );
            save_graph_at(&root, &graph).unwrap();

            let found = render_source(&mut *paint, &mut sim, &mut state, &device, &gen, 32, 18);
            assert!(
                found.is_some(),
                "a generator the graph composes but the pool never saw must render"
            );
            let missing = render_source(&mut *paint, &mut sim, &mut state, &device, "nope", 32, 18);
            assert!(missing.is_none(), "an unknown layer still answers nothing");

            let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        }

        /// A focused `gen.*` that no output composes (not wired) still draws
        /// its own picture — the floating preview used to stay empty for it.
        #[test]
        fn render_source_finds_an_unwired_generator() {
            let device = format!("source-unwired-{}", std::process::id());
            let root = output::data_root();
            let mut paint = crate::paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.paused_at = Some(3.0);
            let ids = paint.ids();

            let mut graph = metal_graph::Graph::new(
                &device_graph_id(&device),
                &device,
                metal_graph::Domain::Live,
            );
            graph.add_node("out", "output.device", [800.0, 0.0]);
            let gen = graph.add_node("gen-0", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            save_graph_at(&root, &graph).unwrap();

            let found = render_source(&mut *paint, &mut sim, &mut state, &device, &gen, 32, 18);
            assert!(found.is_some(), "an unwired generator must still render");

            let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        }

        /// LAY.7c: the mixer. A wired `gen → compose → output.device` graph
        /// resolves to the same member list as the same graph without the
        /// mixer — the walk descends into the compose node's inputs — and the
        /// frame is the same pixels, because the node *is* the mixing step:
        /// its inputs are the members, its output the mixed frame. Removing a
        /// member at the mixer changes the frame.
        ///
        /// Writes to the repo's own `data/graphs/` under a device id unique to
        /// this process, same as the LAY.7b test above, and removes it again.
        #[test]
        fn a_wired_compose_node_is_the_mixer_and_its_inputs_are_the_members() {
            let device = format!("compose-mixer-{}", std::process::id());
            let root = output::data_root();
            let mut paint = crate::paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.paused_at = Some(3.0);

            let mut a = Layer::new("layer-a", "a", 0);
            a.outputs = vec![device.clone()];
            let mut b = Layer::new("layer-b", "b", 1);
            b.outputs = vec![device.clone()];
            state.layers = vec![a, b];
            state.sources.insert(device.clone(), layers::Device::default());
            let ids = paint.ids();

            // The same two members, once through the mixer and once straight
            // into the output — §2.1's N-in/1-out, and its absence.
            let build = |composed: bool| -> metal_graph::Graph {
                let mut g = metal_graph::Graph::new(
                    &device_graph_id(&device),
                    &device,
                    metal_graph::Domain::Live,
                );
                let out = g.add_node("out", "output.device", [800.0, 0.0]);
                let comp = g.add_node("comp", "compose.layers", [-200.0, 0.0]);
                let gen_a = g.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
                g.node_mut(&gen_a).unwrap().config =
                    metal_graph::Config::from_json_str(&json!({ "source_id": "layer-a" }).to_string())
                        .unwrap();
                let gen_b = g.add_node("gen", &format!("gen.{}", ids[1]), [0.0, 100.0]);
                g.node_mut(&gen_b).unwrap().config =
                    metal_graph::Config::from_json_str(&json!({ "source_id": "layer-b" }).to_string())
                        .unwrap();
                let sink = if composed { comp.clone() } else { out.clone() };
                g.add_edge(
                    metal_graph::Endpoint::new(&gen_a, "image"),
                    metal_graph::Endpoint::new(&sink, "image"),
                );
                g.add_edge(
                    metal_graph::Endpoint::new(&gen_b, "image"),
                    metal_graph::Endpoint::new(&sink, "image"),
                );
                if composed {
                    g.add_edge(
                        metal_graph::Endpoint::new(&comp, "image"),
                        metal_graph::Endpoint::new(&out, "image"),
                    );
                }
                g
            };

            // The walk descends through the mixer, in the mixer's edge order.
            let graph = build(true);
            let gens = authored_generators(&graph);
            assert_eq!(gens.len(), 2, "the walk must descend into the mixer");
            let first_gen = graph
                .nodes
                .iter()
                .find(|n| n.config.to_json_str().contains("layer-a"))
                .unwrap()
                .id
                .clone();
            assert_eq!(gens[0], first_gen, "the mixer's edge order is the draw order");

            save_graph_at(&root, &graph).unwrap();
            let (mixed, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);

            // The same members, mixed by the device's own final pass: the
            // mixer must not change what the composition resolves to.
            let graph = build(false);
            save_graph_at(&root, &graph).unwrap();
            let (direct, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert_eq!(
                mixed, direct,
                "the mixer must compose the same members to the same frame"
            );
            assert!(mixed.iter().any(|b| *b != 0), "two members must draw something");

            // Removing a member at the mixer changes the frame.
            let mut graph = build(true);
            let gen_b = graph
                .nodes
                .iter()
                .find(|n| n.config.to_json_str().contains("layer-b"))
                .unwrap()
                .id
                .clone();
            graph.remove_node(&gen_b);
            save_graph_at(&root, &graph).unwrap();
            let (single, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert_ne!(
                single, mixed,
                "removing a mixer member must change the frame"
            );

            let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
        }

        /// LAY.8c: the writer. The table's list becomes the graph — one node
        /// per layer keyed by `source_id`, the z order the edge order, the
        /// five overridable fields pinned — and the frame the graph renders is
        /// byte-identical to the pool's synthesis, which is what makes the
        /// hand-off invisible. A second sync with nothing new does not
        /// rewrite the file; a layer unrouted from the device retires its
        /// node.
        #[test]
        fn the_writer_writes_the_table_as_the_graph_and_the_frame_is_the_same() {
            let device = format!("sync-writer-{}", std::process::id());
            let root = output::data_root();
            let mut paint = crate::paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.paused_at = Some(3.0);

            let mut a = Layer::new("layer-a", "a", 0);
            a.outputs = vec![device.clone()];
            let mut b = Layer::new("layer-b", "b", 1);
            b.outputs = vec![device.clone()];
            b.opacity = 0.5;
            b.blend = Blend::Add;
            state.layers = vec![a, b];
            // The field says `Own` too, so the pool path after the graph file
            // is removed resolves the same list the graph held.
            state.sources.insert(
                device.clone(),
                layers::Device {
                    mode: layers::DeviceMode::Own,
                    ..Default::default()
                },
            );

            ensure_device_graph_at(&root, &device, &device);
            let ids = paint.ids();
            sync_device_graph_at(
                &root,
                &device,
                Some(&layers::DeviceMode::Own),
                &state.layers_on(&device),
                &ids,
            )
            .unwrap();

            let graph = load_graph_at(&root, &device_graph_id(&device)).unwrap();
            let gens = authored_generators(&graph);
            assert_eq!(gens.len(), 2, "one node per routed layer");
            let kinds: Vec<String> = gens
                .iter()
                .map(|g| graph.node(g).unwrap().kind.clone())
                .collect();
            assert_eq!(
                kinds,
                vec![format!("gen.{}", ids[0]), format!("gen.{}", ids[1])],
                "the z order is the edge order and the kind is the layer's model"
            );
            let cfg: Value =
                serde_json::from_str(&graph.node(&gens[1]).unwrap().config.to_json_str()).unwrap();
            assert_eq!(cfg["source_id"], "layer-b");
            assert_eq!(cfg["opacity"], 0.5);
            // R22.5b: the mode is written on the output node's own config,
            // not as a separate `compose.*` node.
            let out_cfg: Value = serde_json::from_str(
                &graph
                    .nodes
                    .iter()
                    .find(|n| n.kind == "output.device")
                    .unwrap()
                    .config
                    .to_json_str(),
            )
            .unwrap();
            assert_eq!(out_cfg["mode"]["kind"], "own");
            let path = graph_path_at(&root, &device_graph_id(&device));

            // Nothing new: the file is left byte-identical.
            let before = metal_vfs::read(&path).unwrap();
            sync_device_graph_at(
                &root,
                &device,
                Some(&layers::DeviceMode::Own),
                &state.layers_on(&device),
                &ids,
            )
            .unwrap();
            assert_eq!(
                before,
                metal_vfs::read(&path).unwrap(),
                "a sync with nothing new must not rewrite the graph"
            );

            // The graph renders what the table renders: same pixels through
            // the written graph and through the pool's synthesis.
            let (through_graph, _, _) =
                render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert!(through_graph.iter().any(|b| *b != 0));

            // A layer unrouted from the device retires its node.
            state.layers.retain(|l| l.id != "layer-b");
            sync_device_graph_at(
                &root,
                &device,
                Some(&layers::DeviceMode::Own),
                &state.layers_on(&device),
                &ids,
            )
            .unwrap();
            let graph = load_graph_at(&root, &device_graph_id(&device)).unwrap();
            assert_eq!(authored_generators(&graph).len(), 1, "the retired layer's node goes");
            assert!(!graph.nodes.iter().any(|n| n.label == "b"));

            let (graph_only, _, _) =
                render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            metal_vfs::remove_file(&path).unwrap();
            let (pool_synth, _, _) =
                render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            assert_eq!(
                graph_only, pool_synth,
                "the written graph and the pool's synthesis are the same frame"
            );

            let _ = metal_vfs::remove_file(&path);
        }

        /// LAY.8c, the canvas half: when the mixer is already wired, the
        /// writer's members wire into the mixer (through their own chains),
        /// never past it — and a device with no graph stays without one.
        #[test]
        fn the_writer_wires_members_into_a_wired_mixer_and_leaves_no_graph_alone() {
            let device = format!("sync-mixer-{}", std::process::id());
            let root = output::data_root();
            let mut paint = crate::paint::painter();
            let ids = paint.ids();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.paused_at = Some(3.0);

            let mut a = Layer::new("layer-a", "a", 0);
            a.outputs = vec![device.clone()];
            let mut b = Layer::new("layer-b", "b", 1);
            b.outputs = vec![device.clone()];
            state.layers = vec![a, b];
            state.sources.insert(device.clone(), layers::Device::default());

            let mut graph =
                metal_graph::Graph::new(&device_graph_id(&device), &device, metal_graph::Domain::Live);
            let out = graph.add_node("out", "output.device", [800.0, 0.0]);
            let comp = graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
            let gen_a = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen_a).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "layer-a" }).to_string())
                    .unwrap();
            graph.add_edge(
                metal_graph::Endpoint::new(&gen_a, "image"),
                metal_graph::Endpoint::new(&comp, "image"),
            );
            graph.add_edge(
                metal_graph::Endpoint::new(&comp, "image"),
                metal_graph::Endpoint::new(&out, "image"),
            );
            save_graph_at(&root, &graph).unwrap();

            sync_device_graph_at(
                &root,
                &device,
                Some(&layers::DeviceMode::Own),
                &state.layers_on(&device),
                &ids,
            )
            .unwrap();
            let graph = load_graph_at(&root, &device_graph_id(&device)).unwrap();
            let into_out = graph
                .edges
                .iter()
                .filter(|e| e.to.node == out && e.to.port == "image")
                .count();
            // R22.5b: the mixer is retired and its members re-cable straight
            // onto `output.device.image` — the fan-in the plan asks for.
            assert_eq!(into_out, 2, "both members wire straight into the output");
            assert!(
                !graph
                    .nodes
                    .iter()
                    .any(|n| metal_graph::is_compose_kind(&n.kind)),
                "the legacy mixer node is retired"
            );

            let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));

            // No graph: a no-op, never a canvas minted by the writer.
            sync_device_graph_at(
                &root,
                &device,
                Some(&layers::DeviceMode::Own),
                &state.layers_on(&device),
                &ids,
            )
            .unwrap();
            assert!(
                !metal_vfs::exists(&graph_path_at(&root, &device_graph_id(&device))),
                "the writer must not mint a canvas"
            );
        }

        /// The real `data/collections.json` fixture, frozen: the original
        /// single-fixture gate (`render_frames_are_pixel_identical_through_
        /// the_plan`) as one more scenario in the golden corpus, so the real
        /// file on disk still gets a frame pinned to it.
        #[test]
        fn the_real_empty_1_collection_renders_its_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());

            // `data/collections.json` is git-ignored operator data; the
            // checked-in copy of the fixture is the dash crate's.
            let list: Vec<crate::layers::Collection> = serde_json::from_str(include_str!(
                "../../metal-dash/tests/fixtures/collections.json"
            ))
            .expect("the collections fixture parses");
            let c = list
                .into_iter()
                .find(|c| c.id == "empty-1")
                .expect("the collections fixture must have empty-1");
            state.layers = stamped(c.layers, PREVIEW);
            state.preview.composition = c.composition;
            state.preview.mode = layers::DeviceMode::Collection(c.id.clone());
            state.paused_at = Some(3.0);

            let pixels = check_golden("empty-1", &mut *paint, &mut state, PREVIEW, 64, 36);
            assert!(
                pixels.iter().any(|b| *b != 0),
                "the real collection's frame must not be black"
            );
        }

        /// A canvas that wires a generator and declares no mode composes
        /// itself, even on a device whose stored mode is the automix default.
        #[test]
        fn a_wired_canvas_without_a_mode_wins_over_the_automix_default() {
            let paint = paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.preview.mode = layers::DeviceMode::Automix;
            let manifest_ids = paint.ids();
            let model = manifest_ids[0].to_string();
            let device = "canvas-wired";
            let mut g = metal_graph::Graph::new(&device_graph_id(device), "c", metal_graph::Domain::Live);
            let gen = g.add_node("gen", &format!("gen.{model}"), [0.0, 0.0]);
            let out = g.add_node("out", "output.device", [1.0, 0.0]);
            g.add_edge(
                metal_graph::Endpoint::new(gen.clone(), "image"),
                metal_graph::Endpoint::new(out, "image"),
            );
            save_graph(&g).unwrap();
            let air = state.airing(&mut sim, state.time());
            let (list, _graph, _plan, _nodes, _remote, mode) = compile_device_plan(
                &state.preview,
                true,
                device,
                &state.layers,
                &air,
                state.router.group(),
                0.0,
                &manifest_ids,
                paint.model_count(),
                state.overlays.on(device, state.time()),
                &state.collections,
                None,
                "laptop",
            )
            .expect("a wired canvas must draw");
            assert_eq!(mode, layers::DeviceMode::Own);
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].id, gen);
        }

        /// An authored canvas is the whole picture: a `gen.*` left unwired
        /// draws nothing, instead of the pool's layers cycling underneath.
        #[test]
        fn an_own_canvas_with_an_unwired_generator_draws_nothing_from_the_pool() {
            let paint = paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.layers = fan_out_stack();
            state.preview.mode = layers::DeviceMode::Own;
            let device = "canvas-unwired";
            let mut g = metal_graph::Graph::new(&device_graph_id(device), "c", metal_graph::Domain::Live);
            g.add_node("gen", "gen.turing", [0.0, 0.0]);
            g.add_node("out", "output.device", [1.0, 0.0]);
            save_graph(&g).unwrap();
            let air = state.airing(&mut sim, state.time());
            let manifest_ids = paint.ids();
            let result = compile_device_plan(
                &state.preview,
                true,
                device,
                &state.layers,
                &air,
                state.router.group(),
                0.0,
                &manifest_ids,
                paint.model_count(),
                state.overlays.on(device, state.time()),
                &state.collections,
                None,
                "laptop",
            );
            assert!(result.is_none(), "an unwired canvas must not fall back to the pool");
        }

        /// Step 1 + Step 3's own gate: a stack with real fan-out compiles to
        /// the shared node `the design notes` §10 Conjunto B5 is about —
        /// one `gen.*` for the two layers that would draw the same pixels,
        /// not two nodes that merely look alike. (The pixels themselves are
        /// `automix`/`fixed-effects`' business; this is the graph's.)
        #[test]
        fn a_fan_out_stack_folds_two_layers_onto_one_generator() {
            let mut paint = crate::paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());

            state.layers = fan_out_stack();
            state.preview.composition = Composition::Overlap;
            state.preview.mode = layers::DeviceMode::Own;
            state.paused_at = Some(3.0);

            // The actual dedup: `back` and `echo` must resolve to the same
            // node, `base_layer` to a different one.
            let air = state.airing(&mut sim, state.time());
            let manifest_ids = paint.ids();
            let (list, _graph, _plan, node_of_layer, _remote, _mode) = compile_device_plan(
                &state.preview,
                true,
                PREVIEW,
                &state.layers,
                &air,
                state.router.group(),
                0.0,
                &manifest_ids,
                paint.model_count(),
                // Nothing playing: these two are about the dedup key and the draw
                // order, and an overlay would change the first of those on
                // purpose. `overlays_do_not_fold_two_layers_onto_one_generator`
                // below is the one that arms one.
                state.overlays.on(PREVIEW, state.time()),
                &state.collections,
                // Nothing scheduled — none of these tests arm a `sequence::Run`.
                None,
                "laptop",
            )
            .expect("a non-empty stack must compile");
            let idx_of = |id: &str| list.iter().position(|l| l.id == id).unwrap();
            assert_eq!(
                node_of_layer[idx_of("back")],
                node_of_layer[idx_of("echo")],
                "two layers sharing model/feed/base must share one gen.* node"
            );
            assert_ne!(
                node_of_layer[idx_of("back")],
                node_of_layer[idx_of("base")],
                "a layer on a different model must not share the shared pair's node"
            );
        }

        /// FNV-1a over the frame's RGBA bytes — the golden corpus's hash.
        /// Written out rather than pulled in: the point is a value a human
        /// can paste, and a 64-bit hash of a 64x36 frame is small enough to
        /// freeze by hand.
        fn frame_hash(px: &[u8]) -> u64 {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in px {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h
        }

        /// The corpus's frozen frames, one per scenario, as FNV-1a over the
        /// RGBA bytes — the gate the deleted `render_device_legacy` used to
        /// be, frozen so the reference outlives the code.
        ///
        /// **Frozen on the CPU painter only, and that is the honest split.**
        /// `Stand` is pure Rust over the row and the clock, so its bytes are
        /// the same on any machine this repo builds on. The wgpu painters are
        /// not — driver, tile size and float path all move pixels — so a
        /// stored GPU hash would be a test that passes only on the machine
        /// that froze it, which is worse than no test. A GPU build runs the
        /// same scenarios through [`check_golden`] and gets determinism plus
        /// non-blackness; the *composition* logic (order, blend, sectors,
        /// masks, overlays, automix, collections) is painter-independent and
        /// is what the CPU table actually pins.
        ///
        /// Re-freezing after a deliberate pixel change: run
        /// `FW_PRINT_GOLDEN=1 cargo test -p metal-perform -- --nocapture` and
        /// paste the printed values.
        const GOLDEN: &[(&str, u64)] = &[
            ("automix", 0xbb66da10d71ce6ad),
            ("collection", 0xa5b7c87e2eba8f89),
            ("contoured", 0xbb66da10d71ce6ad),
            // Re-frozen 2026-09-27 (BLE Phase B): appending the signal block's
            // five floats (`sig_cct`, `sig_zoom`, `sig_focus`, `sig_iris`,
            // `sig_frost`) widens every row and, because the signal block
            // sits *before* the effects, changes the composed row the real
            // `empty-1` fixture renders from. The model's own floats and the
            // effects are read end-relative/name-based and do not move (see
            // metal-schema's `the_signal_block_sits_before_the_effects…` and
            // `an_old_row_is_padded_where_the_effects_begin…`), so this is a
            // deliberate schema widening, not a misread.
            // Re-frozen 2026-10-08 (feat/fx-grade): the grade/post pipeline
            // change alters every composed frame; re-frozen with
            // `FW_PRINT_GOLDEN=1`.
            ("empty-1", 0x9a02de9048308b25),
            ("fixed-effects", 0xcfbefd67fb913a67),
            ("overlay", 0x7a016fced75c4b5c),
            ("remote-ceded", 0x9a02de9048308b25),
            ("scheduled-run", 0xa5b7c87e2eba8f89),
        ];

        /// One corpus scenario's frame, through the live path, checked
        /// against [`GOLDEN`] — and, on every painter, rendered twice so
        /// "the same instant is the same frame" is asserted too (a stored
        /// GPU hash cannot do that half; determinism can).
        ///
        /// `FW_PRINT_GOLDEN=1` prints `GOLDEN <name> <hash>` for re-freezing;
        /// the values in the table were captured through this same call, so
        /// the reference and the gate are one code path.
        fn check_golden(
            name: &str,
            paint: &mut dyn Paint,
            state: &mut State,
            device: &str,
            w: u32,
            h: u32,
        ) -> Vec<u8> {
            let mut sim = crate::listeners::Listeners::stand_in();
            let (plan, pw, ph) = render_device(paint, &mut sim, state, device, w, h);
            assert_eq!((pw, ph), (w, h), "the frame came back the wrong size");
            let hash = frame_hash(&plan);
            if std::env::var("FW_PRINT_GOLDEN").is_ok() {
                eprintln!("GOLDEN {name} {hash:#018x}");
            }
            if !paint.real() {
                let want = GOLDEN
                    .iter()
                    .find(|(n, _)| *n == name)
                    .unwrap_or_else(|| panic!("no golden frame named {name} — add one"))
                    .1;
                assert_eq!(
                    hash, want,
                    "the {name} frame changed; if that is deliberate, re-freeze GOLDEN \
                     (FW_PRINT_GOLDEN=1 prints it)"
                );
            }
            let (again, _, _) = render_device(paint, &mut sim, state, device, w, h);
            assert_eq!(
                plan, again,
                "the same instant rendered twice must be the same frame"
            );
            plan
        }

        fn corpus_collection(id: &str) -> Collection {
            let mut one = Layer::new("corpus-one", "corpus one", 0);
            one.z = 0;
            one.opacity = 0.8;
            one.blend = Blend::Add;
            let mut two = Layer::new("corpus-two", "corpus two", 1);
            two.z = 1;
            two.effects.shake = 0.4;
            two.fijo_mask = 1 << 0;
            Collection {
                id: id.to_string(),
                name: id.to_string(),
                layers: vec![one, two],
                composition: Composition::Overlap,
            }
        }

        #[test]
        fn an_automix_device_renders_its_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            state.layers = stamped(
                vec![
                    Layer::new("a", "a", 0),
                    Layer::new("b", "b", 1),
                    Layer::new("c", "c", 2),
                ],
                PREVIEW,
            );
            state.preview.mode = layers::DeviceMode::Automix;
            state.preview.composition = Composition::Overlap;
            state.paused_at = Some(3.0);
    
            let pixels = check_golden("automix", &mut *paint, &mut state, PREVIEW, 64, 36);
            assert!(
                pixels.iter().any(|b| *b != 0),
                "an automix frame must not be black"
            );
        }

        #[test]
        fn a_collection_device_renders_its_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            state.collections.list.push(corpus_collection("corpus-col"));
            state.preview.mode = layers::DeviceMode::Collection("corpus-col".into());
            // The collection's own composition wins; this one is here to prove
            // the two paths read it off the same place.
            state.preview.composition = Composition::Sectors;
            state.paused_at = Some(3.0);
    
            let pixels = check_golden("collection", &mut *paint, &mut state, PREVIEW, 64, 36);
            assert!(
                pixels.iter().any(|b| *b != 0),
                "a collection frame must not be black"
            );
        }

        #[test]
        fn an_overlay_renders_its_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            state.layers = fan_out_stack();
            state.preview.mode = layers::DeviceMode::Own;
            state.preview.composition = Composition::Overlap;
            state.paused_at = Some(3.0);
            let row = paint.schemas()[0].defaults();
            state.overlays.start(overlay::Play {
                device: PREVIEW.into(),
                layer: "back".into(),
                movement: "corpus".into(),
                model: 0,
                ticks: vec![row],
                started: 0.0,
                looping: true,
                mask: None,
            });
    
            check_golden("overlay", &mut *paint, &mut state, PREVIEW, 64, 36);
        }

        #[test]
        fn a_scheduled_run_renders_its_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            state.collections.list.push(corpus_collection("corpus-run"));
            // Nothing of this device's own to draw: the run is the only thing
            // that can put pixels on the screen, which is the case the empty-list
            // early return used to get wrong in one of the two paths.
            state.layers.clear();
            state.preview.mode = layers::DeviceMode::Own;
            state.runs.start(sequence::Run::live(
                PREVIEW.into(),
                "corpus-run".into(),
                0.0,
            ));
            state.paused_at = Some(3.0);
    
            let pixels = check_golden("scheduled-run", &mut *paint, &mut state, PREVIEW, 64, 36);
            assert!(
                pixels.iter().any(|b| *b != 0),
                "a scheduled run must not be black"
            );
        }

        #[test]
        fn a_contoured_layer_renders_its_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut out = Output::new("corpus-out", "corpus", 64, 36);
            out.masks.push(Mask {
                id: "corpus-mask".into(),
                name: "corpus mask".into(),
                points: vec![[0.1, 0.1], [0.9, 0.1], [0.5, 0.9]],
                ..Mask::default()
            });
            state.outputs.list.push(out);
            state
                .sources
                .insert("corpus-out".into(), layers::Device::default());
            let mut one = Layer::new("one", "one", 0);
            one.outputs = vec!["corpus-out".into()];
            one.mask = Some("corpus-mask".into());
            let mut two = Layer::new("two", "two", 1);
            two.outputs = vec!["corpus-out".into()];
            two.z = 1;
            state.layers = vec![one, two];
            state.paused_at = Some(3.0);
    
            let pixels = check_golden("contoured", &mut *paint, &mut state, "corpus-out", 64, 36);
            assert!(
                pixels.iter().any(|b| *b != 0),
                "a contoured frame must not be black"
            );
        }

        #[test]
        fn a_remote_layer_and_a_ceded_screen_render_their_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut out = Output::new("corpus-out", "corpus", 64, 36);
            out.ceded_to = Some("resolume".into());
            state.outputs.list.push(out);
            state
                .sources
                .insert("corpus-out".into(), layers::Device::default());
            let mut here = Layer::new("here", "here", 0);
            here.outputs = vec!["corpus-out".into()];
            let mut there = Layer::new("there", "there", 1);
            there.outputs = vec!["corpus-out".into()];
            there.render_on = layers::RenderOn::Node("other-node".into());
            state.layers = vec![here, there];
            state.paused_at = Some(3.0);
    
            let pixels = check_golden("remote-ceded", &mut *paint, &mut state, "corpus-out", 64, 36);
            assert!(
                pixels
                    .chunks_exact(4)
                    .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0),
                "a ceded screen must be black on both paths"
            );
        }

        /// A remote layer is composed at its own place in the stack, with its
        /// opacity, through the local compositor: under a local layer it is
        /// hidden by that layer; over a local layer it is mixed in at its opacity.
        #[test]
        fn a_remote_layer_composes_at_its_stack_place_with_its_opacity() {
            fn rig(paint: &mut dyn Paint, far_z: usize, far_opacity: f32, above: bool) -> State {
                let mut state = State::new(&*paint);
                state.outputs.list.push(Output::new("corpus-out", "corpus", 64, 36));
                // Own mode: the device's layers are the ones on it, not an automix pool.
                let device = layers::Device { mode: layers::DeviceMode::Own, ..Default::default() };
                state.sources.insert("corpus-out".into(), device);
                let mut low = Layer::new("low", "low", 0);
                low.outputs = vec!["corpus-out".into()];
                let mut far = Layer::new("far", "far", far_z);
                far.outputs = vec!["corpus-out".into()];
                far.render_on = layers::RenderOn::Node("other-node".into());
                far.opacity = far_opacity;
                let mut layers_now = vec![low, far];
                if above {
                    let mut top = Layer::new("top", "top", 9);
                    top.outputs = vec!["corpus-out".into()];
                    layers_now.push(top);
                }
                state.layers = layers_now;
                state.paused_at = Some(3.0);
                state
            }
            // A remote layer that is always white, asked for once per frame.
            let white = |ask: &RemoteAsk| {
                assert_eq!(ask.node, "other-node");
                Some((64, 36, vec![255u8; 64 * 36 * 3]))
            };

            // Remote on top of the local layer, at half opacity.
            let mut paint = crate::paint::painter();
            let mut state = rig(&mut *paint, 5, 0.5, false);
            let mut sim = crate::listeners::Listeners::stand_in();
            let (local, w, h) = render_device(&mut *paint, &mut sim, &mut state, "corpus-out", 64, 36);
            let mut sim = crate::listeners::Listeners::stand_in();
            let calls = std::cell::Cell::new(0);
            let (mixed, _, _) = render_device_remote(&mut *paint, &mut sim, &mut state, "corpus-out", 64, 36, &mut |a| {
                calls.set(calls.get() + 1);
                white(a)
            });
            assert_eq!(calls.get(), 1, "the remote layer is asked for once");
            assert_eq!((w, h), (64, 36));
            for (l, m) in local.chunks_exact(4).zip(mixed.chunks_exact(4)) {
                for c in 0..3 {
                    let want = (l[c] as f32 + 255.0) * 0.5;
                    assert!((m[c] as f32 - want).abs() <= 2.0, "opacity 0.5 over the local frame: {} vs {want}", m[c]);
                }
            }

            // Remote under a full local layer: the local layer hides it.
            let mut paint = crate::paint::painter();
            let mut state = rig(&mut *paint, 0, 1.0, true);
            let mut sim = crate::listeners::Listeners::stand_in();
            let (covered, _, _) =
                render_device_remote(&mut *paint, &mut sim, &mut state, "corpus-out", 64, 36, &mut |a| white(a));
            let mut sim = crate::listeners::Listeners::stand_in();
            let (only_top, _, _) = render_device(&mut *paint, &mut sim, &mut state, "corpus-out", 64, 36);
            assert_eq!(covered, only_top, "a remote layer below a full local layer is not seen");
        }

        /// A remote layer goes through the same pipeline as a local one: its
        /// `fx.*` chain is resolved from the graph and sent with the state, the
        /// peer applies it, and the venue surface warps it. Drawn on another
        /// machine (a second painter standing in for the peer), the composed
        /// frame matches the one the layer makes here, pixel for pixel, at
        /// every show time in [`PARITY_TIMES`].
        #[test]
        fn a_remote_layer_matches_the_same_layer_drawn_here_through_fx_and_warp() {
            for t in PARITY_TIMES {
                let worst = parity_worst_at(t);
                assert!(worst <= 2, "remote and local frames differ by {worst} at show time {t}");
            }
        }

        /// The show times the parity test is checked at.
        const PARITY_TIMES: [f32; 5] = [0.0, 1.25, 3.0, 3.5, 7.9];

        /// Draws the parity layer here and on the peer at show time `t`, each
        /// from a fresh state, and returns the worst per-channel difference.
        fn parity_worst_at(t: f32) -> i32 {
            use metal_graph::{Domain, Endpoint, Graph};
            let device = format!("remote-match-{}", std::process::id());
            let root = output::data_root();
            let mut paint = paint::painter();
            let mut peer = paint::painter();
            let mut state = State::new(paint.as_ref());
            state.outputs.list.push(Output::new(&device, "remote-match", 64, 64));
            state.sources.insert(device.clone(), layers::Device::default());
            let mut layer = Layer::new("remote-layer", "remote layer", 0);
            layer.outputs = vec![device.clone()];
            layer.fondo_alpha = true;
            let mut base = state.base[0].clone();
            base[0] = 0.9;
            layer.base = Some(base);
            state.layers = vec![layer];

            // The same venue fixture the surface test uses, under its own id.
            let toml = format!(
                r#"
[venue]
id = "remote-match"
name = "Remote match"
setting = "indoor"

[[surface]]
id = "front"
name = "Front"
origin = [-1.0, 1.0, 0.0]
u = [2.0, 0.0, 0.0]
v = [0.0, -2.0, 0.0]

[[object]]
id = "table"
name = "Table"
kind = "table"
at = [0.0, 0.0, 2.0]
size = [1.0, 0.75, 1.0]

[[projector]]
id = "proj"
name = "Proj"
output = "{device}"
on = "table"
yaw = 0.0
pitch = -10.0
throw_ratio = 1.5

[projector.pins]
front = [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]]
"#
            );
            std::fs::create_dir_all(crate::venue::dir(&root)).unwrap();
            std::fs::write(crate::venue::dir(&root).join("remote-match.toml"), &toml).unwrap();

            // gen → fx.speed → fx.posterize → venue.surface → output, as an
            // operator wires it: the chain past the generator, the warp past it.
            let ids = paint.ids();
            let mut graph = Graph::new(&device_graph_id(&device), &device, Domain::Live);
            let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
            let gen = graph.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "source_id": "remote-layer" }).to_string()).unwrap();
            let fx = graph.add_node("fx", "fx.posterize", [200.0, 0.0]);
            graph.node_mut(&fx).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "fixed": 0.6 }).to_string()).unwrap();
            let surface = graph.add_node("surface", metal_graph::VENUE_SURFACE_KIND, [400.0, 0.0]);
            graph.node_mut(&surface).unwrap().config = metal_graph::Config::from_json_str(
                &json!({ "venue": "remote-match", "surface": "front" }).to_string(),
            )
            .unwrap();
            let speed = graph.add_node("speed", "fx.speed", [100.0, 0.0]);
            graph.node_mut(&speed).unwrap().config =
                metal_graph::Config::from_json_str(&json!({ "fixed": 0.5 }).to_string()).unwrap();
            graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&speed, "image"));
            graph.add_edge(Endpoint::new(&speed, "image"), Endpoint::new(&fx, "image"));
            graph.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&surface, "image"));
            graph.add_edge(Endpoint::new(&surface, "image"), Endpoint::new(&out_node, "image"));

            // A script answers `w = 1.0` and its output is bound to the
            // generator's first float, so the row the peer draws carries a
            // bound value and a script output, not the generator's own default.
            let script = graph.add_node("script", "script.py", [-200.0, 80.0]);
            graph.node_mut(&script).unwrap().config = metal_graph::Config::from_json_str(
                &json!({
                    "code": "def process(floats, row):\n    return [floats[0]]\n",
                    "floats": [{ "name": "w", "min": 0.0, "max": 1.0, "default": 1.0 }]
                })
                .to_string(),
            )
            .unwrap();
            let floats: Vec<String> = {
                let schemas = paint.schemas();
                let gp = crate::bind::gen_params(&graph, &ids, &schemas);
                gp[&gen]
                    .iter()
                    .filter(|p| p.kind == metal_graph::params::ParamKind::Float)
                    .map(|p| p.name.clone())
                    .collect()
            };
            let bound = floats.first().expect("the model has a float to bind").clone();
            graph.add_edge(Endpoint::new(&script, "control.w"), Endpoint::new(&gen, &format!("param:{bound}")));
            // A JavaScript script on the second float, when `node` is here to
            // run it: the other runtime a script node can name, and its output
            // has to reach the peer's row the same way the Python one's does.
            let node_here = std::process::Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success());
            if node_here {
                let second = floats.get(1).expect("the model has a second float for the JS script");
                let js = graph.add_node("js", "script.js", [-200.0, 160.0]);
                graph.node_mut(&js).unwrap().config = metal_graph::Config::from_json_str(
                    &json!({
                        "code": "function process(floats, row) { return [floats[0] * 0.5]; }",
                        "floats": [{ "name": "k", "min": 0.0, "max": 1.0, "default": 0.7 }]
                    })
                    .to_string(),
                )
                .unwrap();
                graph.add_edge(Endpoint::new(&js, "control.k"), Endpoint::new(&gen, &format!("param:{second}")));
            }
            save_graph_at(&root, &graph).unwrap();
            // The venue and graph the test writes go to the per-process
            // scratch dirs the test fences hand out, never `data/` itself.
            assert_ne!(crate::venue::dir(&root), root.join("venues"), "the venue fixture must not land in data/");
            assert_ne!(crate::engine::graphs_dir(&root), root.join("graphs"), "the graph must not land in data/");

            // Warm the fx.speed clock one second behind `t`, so at `t` it has
            // fallen behind the show clock (a clock step is capped at 1 s).
            state.paused_at = Some((t - 1.0).max(0.0));
            let mut sim = crate::listeners::Listeners::stand_in();
            render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
            state.paused_at = Some(t);

            // Drawn here.
            let (here, w, h) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
            assert_eq!((w, h), (64, 64));
            assert!(here.chunks_exact(4).any(|p| p[0..3].iter().any(|b| *b != 0)), "the local layer draws");

            // Drawn on the peer: the layer is pinned to another machine, the
            // peer draws the state and chain it is sent and nothing else.
            state.layers[0].render_on = layers::RenderOn::Node("peer".into());
            let mut asked = 0;
            let mut seen = (0.0f32, false);
            let mut sim = crate::listeners::Listeners::stand_in();
            let (there, _, _) = render_device_remote(&mut *paint, &mut sim, &mut state, &device, 64, 64, &mut |ask| {
                asked += 1;
                seen = (ask.time, ask.fondo_alpha);
                assert_eq!(ask.node, "peer");
                assert_eq!(ask.chain.len(), 1, "the posterize travels with the ask");
                let (px, pw, ph) = peer.solo_chain(ask.model, ask.row, ask.time, ask.w, ask.h, ask.fondo_alpha, ask.chain);
                let rgb = px.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
                Some((pw, ph, rgb))
            });
            assert_eq!(asked, 1, "the remote layer is asked for once");
            assert!(seen.1, "fondo_alpha travels with the ask");

            let worst = here
                .iter()
                .zip(there.iter())
                .map(|(a, b)| (*a as i32 - *b as i32).abs())
                .max()
                .unwrap_or(0);
            let lit = |px: &[u8], x: u32, y: u32| {
                let i = ((y * 64 + x) * 4) as usize;
                px[i..i + 3].iter().any(|b| *b != 0)
            };
            assert!(lit(&there, 32, 32), "the pinned centre is lit on the remote path");
            assert!(!lit(&there, 2, 2), "the warp cuts the remote layer to its pins");
            worst
        }

        #[test]
        fn per_layer_fixed_effects_render_their_golden_frame() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut fixed = Layer::new("fixed", "fixed", 0);
            fixed.effects.shake = 0.6;
            fixed.fijo_mask = 1 << 0;
            // Same model, same (absent) base — the pair `GenKey` used to fold
            // onto one generator while they disagreed about a fixed effect.
            let reactive = Layer::new("reactive", "reactive", 0);
            state.layers = stamped(vec![fixed, reactive], PREVIEW);
            state.preview.mode = layers::DeviceMode::Own;
            state.preview.composition = Composition::Overlap;
            state.paused_at = Some(3.0);
    
            let pixels = check_golden("fixed-effects", &mut *paint, &mut state, PREVIEW, 64, 36);
            assert!(
                pixels.iter().any(|b| *b != 0),
                "a per-layer-effects frame must not be black"
            );
    
            // And the fold itself is now honest: the two must NOT share a node.
            let air = state.airing(&mut Input::stand_in(), state.time());
            let manifest_ids = paint.ids();
            let (list, _graph, _plan, node_of_layer, _remote, _mode) = compile_device_plan(
                &state.preview,
                true,
                PREVIEW,
                &state.layers,
                &air,
                state.router.group(),
                0.0,
                &manifest_ids,
                paint.model_count(),
                state.overlays.on(PREVIEW, state.time()),
                &state.collections,
                None,
                "laptop",
            )
            .expect("a non-empty stack must compile");
            let idx_of = |id: &str| list.iter().position(|l| l.id == id).unwrap();
            assert_ne!(
                node_of_layer[idx_of("fixed")],
                node_of_layer[idx_of("reactive")],
                "layers that disagree about a fixed effect must not share one gen.* node"
            );
        }

        /// The other half of "byte for byte": a frame is a function of its
        /// inputs. Rendering the same paused instant twice through the live path
        /// must return the same pixels — the property every digest, router and
        /// trail gate depends on.
        #[test]
        fn the_same_instant_renders_the_same_frame_twice() {
            let mut paint = crate::paint::painter();
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.layers = fan_out_stack();
            state.preview.mode = layers::DeviceMode::Own;
            state.preview.composition = Composition::Overlap;
            state.paused_at = Some(3.0);
    
            let (first, w, h) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
            let (second, w2, h2) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
            assert_eq!((w, h), (w2, h2));
            assert_eq!(first, second, "the same instant must render the same frame");
        }

        /// LAY.5's own gate: an effect step at amount 0 must leave the frame
        /// untouched. The chain is walked from data now, so the property that
        /// makes every default row safe — "off" is a `mix` that returns its own
        /// input — has to hold for every declared step, including the
        /// footage promotion whose `mix` is all `amt` gates.
        #[test]
        fn a_zero_amount_chain_renders_the_same_frame_as_no_chain() {
            let mut paint = crate::paint::painter();
            let row = paint.schemas()[0].defaults();
            let mut chain: Vec<metal_schema::EffectStep> = metal_schema::EFFECT_SPECS
                .iter()
                .enumerate()
                .filter(|(_, s)| s.chains)
                .map(|(i, _)| metal_schema::EffectStep {
                    effect: i as u32,
                    amount: 0.0,
                    slot: 0,
                })
                .collect();
            chain.push(metal_schema::EffectStep {
                effect: metal_schema::CHAIN_KIND_ESTELA,
                amount: 0.0,
                slot: 0,
            });
    
            let plain = paint.solo(0, &row, 1.0, 64, 36, false);
            let zeroed = paint.solo_chain(0, &row, 1.0, 64, 36, false, &chain);
            assert_eq!(
                plain, zeroed,
                "a chain of zero-amount steps must render the same frame as no chain at all"
            );
        }

        /// Lay `beats` beats of digest history into `state`'s one Looper at the
        /// router's own 8 Hz, and engage a `len`-beat manual loop there.
        ///
        /// At 120 BPM a beat is half a second, so an 8 Hz poll advances a
        /// quarter beat and every beat the ring records lands exactly on a
        /// router tick — which is what lets the tests below pick an instant
        /// that is *both* a legal `poll` time and a legal `observe` tick.
        fn lay_a_loop(state: &mut State, sim: &mut Input, beats: u32, len: u32) -> (f64, f32) {
            sim.set_bpm(120.0);
            let mut t = 0.0f32;
            while t <= (beats as f32) / 2.0 {
                state.looper.poll(sim, t);
                t += 1.0 / 8.0;
            }
            state.looper.lock(Some(len));
            let since = state
                .looper
                .status()
                .engaged
                .expect("a manual lock must engage")
                .since;
            (since, 2.0)
        }

        /// Seconds at `beat` on a 120 BPM clock.
        fn at(beat: f64) -> f32 {
            beat as f32 / 2.0
        }

        /// The router and the row must read the *same* frame at one instant.
        ///
        /// The router is fed its frame through `looper.poll`; the row path is
        /// fed the same `looper.poll` at the same `time`. Because the replay is
        /// indexed by beat phase (not by call count), the router's own poll
        /// cannot move the row's — so a `poll` at the instant the router used,
        /// after `on_air`, must return exactly what a `poll` there returned
        /// before it.
        #[test]
        fn the_router_and_the_row_see_the_same_frame() {
            let paint = paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut sim = crate::listeners::Listeners::stand_in();
            let (since, _) = lay_a_loop(&mut state, &mut sim, 16, 4);

            // A full loop past the anchor, so the replay is genuinely a
            // recorded row and not the live one the first cycle would show.
            // `since + 5` beats is an 8 Hz tick, which is the instant the
            // router's own `observe` will poll.
            let probe = at(since + 5.0);

            let row_before = state.looper.poll(&mut sim, probe);
            // The router observes the looped frame here (its closure calls the
            // same `looper.poll`).
            let _ = state.on_air(&mut sim, probe);
            let row_after = state.looper.poll(&mut sim, probe);
            assert_eq!(
                row_before.f, row_after.f,
                "the router's observation must not move the row's frame"
            );
            assert_eq!(row_before.raw, row_after.raw);

            // And the shared frame really is the replay, not the live digest —
            // otherwise "the same frame" would be trivially true in
            // pass-through.
            let live = sim.poll(probe);
            assert_ne!(
                row_before.f, live.f,
                "an engaged loop must give the row a replayed frame, not the live one"
            );
        }

        /// While a loop is engaged, the row driven at two beat-phase-equal
        /// instants must be identical — the whole point of indexing the replay
        /// by phase (plan decision 5). Drives a real model schema's row through
        /// `drive::row`, not just the `Frame`, so "the driven rows match" is
        /// the thing asserted.
        #[test]
        fn drive_rows_repeat_exactly_while_engaged() {
            let paint = paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut sim = crate::listeners::Listeners::stand_in();
            let (since, _) = lay_a_loop(&mut state, &mut sim, 16, 4);

            // One beat in, and the same phase one loop length later.
            let frame_one = state.looper.poll(&mut sim, at(since + 1.0));
            let frame_five = state.looper.poll(&mut sim, at(since + 5.0));
            assert_eq!(
                frame_one.f, frame_five.f,
                "the same beat phase one loop apart must be the same frame"
            );

            let schema = &paint.schemas()[0];
            let base = schema.defaults();
            let row_one = drive::row(schema, &base, &frame_one, &Drive::default());
            let row_five = drive::row(schema, &base, &frame_five, &Drive::default());
            assert_eq!(
                row_one, row_five,
                "a model driven on a by-phase replay must repeat with the loop"
            );
        }

        /// The manual loop, end to end through `State`: no detector is ever
        /// fed, and a black frame or an unengaged loop would both fail. (The
        /// Looper's own test covers the arithmetic; this is the wiring.)
        #[test]
        fn the_manual_loop_works_without_a_detector() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut sim = crate::listeners::Listeners::stand_in();
            let (since, _) = lay_a_loop(&mut state, &mut sim, 16, 8);

            let status = state.looper.status();
            assert_eq!(
                status.engaged.map(|e| e.len),
                Some(8),
                "locking the last eight beats must engage with no detector"
            );
            assert_eq!(
                status.detector, None,
                "the stand-in has no loop set, so no detector ever fired"
            );

            // A real render through the live path while engaged: it must come
            // back the asked-for size and not panic. The manual loop survives
            // `render_device`'s own detector feed (which is `None`).
            state.layers = fan_out_stack();
            state.preview.mode = layers::DeviceMode::Own;
            state.preview.composition = Composition::Overlap;
            state.paused_at = Some(at(since + 5.0));
            let (pixels, w, h) =
                render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
            assert_eq!((w, h), (64, 36));
            assert_eq!(pixels.len(), (64 * 36 * 4) as usize);
            assert_eq!(
                state.looper.status().engaged.map(|e| e.len),
                Some(8),
                "the manual loop must still be engaged after a render"
            );
        }

        // -- S.AM: the ai.colourist node ------------------------------------

        /// A scratch reels root with a fitted grade per `(model, schema)`: the
        /// bias on float 0 is `bias`, every other float is zero, so the fitted
        /// row is distinguishable from the round robin and from another
        /// model's fit by the sign of its first entry.
        fn ai_reels(name: &str, models: &[(&str, &'static metal_schema::Schema, f32)]) -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!("fw-ai-colourist-{name}-{}", std::process::id()));
            let _ = metal_vfs::remove_dir_all(&dir);
            for (model, schema, bias) in models {
                let mut offsets = vec![0.0f32; metal_reel::grade::float_names(schema).len()];
                offsets[0] = *bias;
                let takes = vec![metal_reel::grade::Take {
                    song: "s".into(),
                    music: [0.5f32; metal_reel::grade::INPUTS],
                    offsets,
                }];
                let grade = metal_reel::grade::train(model, schema, &takes);
                metal_reel::grade::Colourist::from_grade(grade).save(&dir).expect("saves the grade");
            }
            dir
        }

        /// `audio.live → ai.colourist → gen.<model>.control`, once per model,
        /// all fed by the one `ai.*` node — the fan-out shape.
        fn ai_graph(name: &str, models: &[&str]) -> (metal_graph::Graph, Vec<String>) {
            use metal_graph::{Domain, Endpoint, Graph};
            let mut g = Graph::new(name, name, Domain::Live);
            let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
            let ai = g.add_node("ai", metal_graph::AI_COLOURIST_KIND, [1.0, 0.0]);
            g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&ai, "control"));
            let mut gens = Vec::new();
            for (i, model) in models.iter().enumerate() {
                let gen = g.add_node("gen", &format!("gen.{model}"), [2.0, i as f32 * 40.0]);
                g.add_edge(Endpoint::new(&ai, "control"), Endpoint::new(&gen, "control"));
                gens.push(gen);
            }
            (g, gens)
        }

        /// T7: a generator's `loop` config — absent (default factory loop),
        /// `"none"` (exactly today's drive) and an explicit id.
        #[test]
        fn a_generator_loop_choice_overrides_only_what_the_loop_targets() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let ids = paint.ids();
            let model = ids[0];
            let schema = paint.schemas()[0];
            let metal_schema::Block::Floats(specs) = schema
                .blocks
                .iter()
                .find(|b| matches!(b, metal_schema::Block::Floats(s) if !s.is_empty()))
                .expect("a model with floats")
            else {
                unreachable!()
            };
            let at = schema.blocks.iter().take_while(|b| !matches!(b, metal_schema::Block::Floats(s) if !s.is_empty())).map(|b| b.width()).sum::<usize>();
            let param = specs[0].name;
            let dir = std::env::temp_dir().join(format!("fw-engine-loops-{}", std::process::id()));
            let _ = metal_vfs::remove_dir_all(&dir);
            metal_vfs::create_dir_all(dir.join("factory")).unwrap();
            let steps = "[1,null,null,null,0,null,null,null,null,null,null,null,null,null,null,null]";
            for id in ["a-first", "b-second"] {
                metal_vfs::write(
                    dir.join("factory").join(format!("{id}.json")),
                    format!(r#"{{"id":"{id}","name":"x","target":{{"model":"{model}"}},"tracks":[{{"target":{{"kind":"float","param":"{param}"}},"steps":{}}}]}}"#,
                        if id == "a-first" { steps.to_string() } else { steps.replace("[1,", "[0.5,") }),
                )
                .unwrap();
            }
            state.loops = crate::pattern::Library::load(&dir.join("factory"), &dir.join("user"));
            assert!(state.loops.problems.is_empty(), "{:?}", state.loops.problems);
            state.music_pos = crate::clock::Position::from_beats(0.0);

            let base = schema.defaults();
            let frame = Input::stand_in().poll(0.5);
            let plain = drive::row(schema, &base, &frame, &Drive::default());
            let mut g = metal_graph::Graph::new("loops", "loops", metal_graph::Domain::Live);
            let gen = g.add_node("gen", &format!("gen.{model}"), [0.0, 0.0]);
            let mut row_with_config = |state: &mut State, cfg: Option<&str>| {
                let n = g.nodes.iter_mut().find(|n| n.id == gen).unwrap();
                n.config = cfg.map(|c| metal_graph::Config::from_json_str(c).unwrap()).unwrap_or_default();
                row_for_node(Some(&g), Some(&gen), &mut *paint, state, 0, &base, Some(&frame), 0.5, None, None)
            };

            let none = row_with_config(&mut state, Some(r#"{"loop":"none"}"#));
            assert_eq!(none, plain, "\"none\" is today's drive exactly");
            let default = row_with_config(&mut state, None);
            assert_eq!(default[at], crate::pattern::to_range(1.0, &specs[0]), "default = first factory loop");
            let mut rest = default.clone();
            rest[at] = plain[at];
            assert_eq!(rest, plain, "untargeted params keep the audio drive");
            let second = row_with_config(&mut state, Some(r#"{"loop":"b-second"}"#));
            assert_eq!(second[at], crate::pattern::to_range(0.5, &specs[0]));
            state.music_pos = crate::clock::Position::from_beats(1.0);
            let later = row_with_config(&mut state, None);
            assert_eq!(later[at], crate::pattern::to_range(0.0, &specs[0]), "step 4");
            assert_eq!(later, row_with_config(&mut state, None), "same tick, same row");
            let unknown = row_with_config(&mut state, Some(r#"{"loop":"nope"}"#));
            assert_eq!(unknown, plain);
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        /// T8: an `fx.*` node's `loop` — default factory loop follows its
        /// steps at known positions, a wired `control` beats it, `"none"` is
        /// today's Fixed exactly.
        #[test]
        fn an_effect_loop_drives_the_amount_unless_wired_or_none() {
            use metal_graph::effects::{set_effect, ControlSource};
            use metal_graph::Endpoint;
            let dir = std::env::temp_dir().join(format!("fw-engine-fx-loops-{}", std::process::id()));
            let _ = metal_vfs::remove_dir_all(&dir);
            metal_vfs::create_dir_all(dir.join("factory")).unwrap();
            metal_vfs::write(
                dir.join("factory").join("fx-vignette-a.json"),
                r#"{"id":"fx-vignette-a","name":"x","target":{"effect":"vignette"},"tracks":[{"target":{"kind":"amount"},"steps":[1,null,null,null,0.5,null,null,null,0,null,null,null,null,null,null,null]}]}"#,
            )
            .unwrap();
            let lib = crate::pattern::Library::load(&dir.join("factory"), &dir.join("user"));
            assert!(lib.problems.is_empty(), "{:?}", lib.problems);
            let idx = metal_schema::effect_index("vignette").unwrap();
            let spec = &metal_schema::EFFECTS_FLOATS[idx];
            let at = |u: f32| spec.min + u * (spec.max - spec.min);

            let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let out = g.add_node("out", "output.device", [3.0, 0.0]);
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
            let fx = set_effect(&mut g, &gen, "vignette", metal_schema::EffectState::Fixed(0.3), ControlSource::RawIngest)
                .unwrap()
                .unwrap();
            let row0 = vec![0.0f32; metal_schema::EFFECTS_WIDTH + 4];
            let amount = |g: &metal_graph::Graph, beats: f64| {
                let mut g = g.clone();
                let mut rows: crate::bind::Rows = [(gen.clone(), row0.clone())].into_iter().collect();
                apply_effect_loops(&mut g, &mut rows, &lib, crate::clock::Position::from_beats(beats));
                let mut post = metal_schema::read_post(&rows[&gen]);
                let chain = crate::resolved_chain(&g, &gen, &mut post, &[]);
                (chain[0].amount, rows[&gen][rows[&gen].len() - metal_schema::EFFECTS_WIDTH + idx])
            };

            // Absent `loop`: the factory loop, step by step (1 beat = 4 steps).
            assert!(effect_loop(&g, &fx, &lib).is_some());
            assert_eq!(amount(&g, 0.0), (at(1.0), at(1.0)));
            assert_eq!(amount(&g, 0.5).0, at(1.0), "held step");
            assert_eq!(amount(&g, 1.0).0, at(0.5));
            assert_eq!(amount(&g, 2.25).0, at(0.0));
            assert_eq!(amount(&g, 2.25), amount(&g, 2.25), "same tick, same amount");

            // "none": exactly today's Fixed.
            let mut none = g.clone();
            none.node_mut(&fx).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"fixed":0.3,"loop":"none"}"#).unwrap();
            assert_eq!(metal_graph::effects::state_of(&none, &fx), metal_schema::EffectState::Fixed(0.3));
            let mut plain = g.clone();
            plain.node_mut(&fx).unwrap().config = metal_graph::effects::FxConfig { fixed: 0.3 }.to_config();
            let fixed_chain = {
                let mut post = metal_schema::read_post(&row0);
                crate::resolved_chain(&plain, &gen, &mut post, &[])
            };
            assert_eq!(amount(&none, 1.0).0, fixed_chain[0].amount);
            assert_eq!(amount(&none, 1.0).0, 0.3);

            // Unknown id plays nothing.
            let mut unknown = g.clone();
            unknown.node_mut(&fx).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"fixed":0.3,"loop":"nope"}"#).unwrap();
            assert_eq!(amount(&unknown, 1.0).0, 0.3);

            // A wired control wins: Reactive, the loop is not consulted.
            let mut wired = g.clone();
            set_effect(&mut wired, &gen, "vignette", metal_schema::EffectState::Reactive, ControlSource::Row).unwrap();
            assert!(effect_loop(&wired, &fx, &lib).is_none());
            let mut rows: crate::bind::Rows = [(gen.clone(), row0.clone())].into_iter().collect();
            apply_effect_loops(&mut wired, &mut rows, &lib, crate::clock::Position::from_beats(0.0));
            assert_eq!(rows[&gen], row0, "a wired effect's row slot is the router's");
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        /// T11: a multi-parameter effect loop writes every param it targets
        /// under its own config key, leaves the rest alone and is pure per
        /// tick (bindings run after it, as in T8, so they still win).
        #[test]
        fn a_multi_param_effect_loop_writes_each_targeted_param() {
            use metal_graph::Endpoint;
            let dir = std::env::temp_dir().join(format!("fw-engine-fx-multi-{}", std::process::id()));
            let _ = metal_vfs::remove_dir_all(&dir);
            metal_vfs::create_dir_all(dir.join("factory")).unwrap();
            metal_vfs::write(
                dir.join("factory").join("fx-move-a.json"),
                r#"{"id":"fx-move-a","name":"x","target":{"effect":"move"},"tracks":[
                    {"target":{"kind":"float","param":"x"},"steps":[1,null,null,null,0,null,null,null,0.5,null,null,null,null,null,null,null]},
                    {"target":{"kind":"float","param":"wrap"},"steps":[0.25,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null]},
                    {"target":{"kind":"float","param":"nope"},"steps":[1,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null]}]}"#,
            )
            .unwrap();
            metal_vfs::write(
                dir.join("factory").join("fx-forge-a.json"),
                r#"{"id":"fx-forge-a","name":"x","target":{"effect":"forge"},"tracks":[
                    {"target":{"kind":"float","param":"onset"},"steps":[1,0,null,null,null,null,null,null,null,null,null,null,null,null,null,null]}]}"#,
            )
            .unwrap();
            let lib = crate::pattern::Library::load(&dir.join("factory"), &dir.join("user"));
            assert!(lib.problems.is_empty(), "{:?}", lib.problems);

            let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let mv = g.add_node("mv", "fx.move", [1.0, 0.0]);
            let fg = g.add_node("fg", "fx.forge", [2.0, 0.0]);
            let out = g.add_node("out", "output.device", [3.0, 0.0]);
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mv, "image"));
            g.add_edge(Endpoint::new(&mv, "image"), Endpoint::new(&fg, "image"));
            g.add_edge(Endpoint::new(&fg, "image"), Endpoint::new(&out, "image"));
            g.node_mut(&mv).unwrap().config = metal_graph::Config::from_json_str(r#"{"y":0.4}"#).unwrap();
            let row0 = vec![0.0f32; metal_schema::EFFECTS_WIDTH + 4];
            let run = |g: &metal_graph::Graph, beats: f64| {
                let mut g = g.clone();
                let mut rows: crate::bind::Rows = [(gen.clone(), row0.clone())].into_iter().collect();
                apply_effect_loops(&mut g, &mut rows, &lib, crate::clock::Position::from_beats(beats));
                assert_eq!(rows[&gen], row0, "a multi-param effect has no row slot");
                g
            };
            let cfg = |g: &metal_graph::Graph, n: &str| -> serde_json::Value {
                serde_json::from_str(&g.node(n).unwrap().config.to_json_str()).unwrap()
            };
            assert!(effect_loop(&g, &mv, &lib).is_some());
            let a = run(&g, 0.0);
            let m = cfg(&a, &mv);
            assert_eq!(m["x"], serde_json::json!(1.0), "x: 1 of -1..1");
            assert_eq!(m["wrap"], serde_json::json!(0.25));
            assert_eq!(m["y"], serde_json::json!(0.4), "untargeted params keep their value");
            assert!(m.get("nope").is_none(), "undeclared names are ignored");
            assert_eq!(cfg(&run(&g, 1.0), &mv)["x"], serde_json::json!(-1.0), "step 4: 0 of -1..1");
            assert_eq!(cfg(&run(&g, 2.0), &mv)["x"], serde_json::json!(0.0));
            assert_eq!(cfg(&run(&g, 1.0), &mv), cfg(&run(&g, 1.0), &mv), "same tick, same values");
            // fx.forge: the pinned ingest column reaches what the shader reads.
            let f = run(&g, 0.0);
            let raw = vec![0.5f32; 16];
            let ingest = crate::forge::fx_ingest(f.node(&fg).unwrap(), &raw);
            assert_eq!((ingest[2], ingest[0]), (1.0, 0.5), "onset pinned, rms the digest's");
            assert_eq!(crate::forge::fx_ingest(run(&g, 0.25).node(&fg).unwrap(), &raw)[2], 0.0);
            // "none" leaves the config exactly as authored.
            let mut none = g.clone();
            none.node_mut(&mv).unwrap().config = metal_graph::Config::from_json_str(r#"{"y":0.4,"loop":"none"}"#).unwrap();
            assert_eq!(cfg(&run(&none, 0.0), &mv), serde_json::json!({"y":0.4,"loop":"none"}));
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        #[test]
        fn an_ai_node_between_audio_and_source_uses_the_colourist() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let ids = paint.ids();
            let model = ids[0];
            let schema = paint.schemas()[0];
            let base = schema.defaults();
            let mut sim = crate::listeners::Listeners::stand_in();
            let frame = sim.poll(0.5);

            let dir = ai_reels("one", &[(model, schema, 1.0)]);
            state.reels = Reels::new(&dir);
            let (g, gens) = ai_graph("ai-one", &[model]);

            let row = row_for_node(
                Some(&g), Some(&gens[0]), &mut *paint, &mut state, 0, &base, Some(&frame), 0.5, None, None,
            );
            let plain = drive::row(schema, &base, &frame, &Drive::default());
            assert_ne!(row, plain, "the fitted colourist must move the row off the round robin");

            let offsets = ai_offsets(Some(&g), Some(&gens[0]), &mut state, schema, model, &frame, 1.0)
                .expect("a trained colourist answers offsets");
            assert_eq!(offsets[0], 1.0, "the fitted bias on float 0");
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        /// `mode: "auto"` keeps the node wired but drives the source with the
        /// deterministic round robin, trained grade or not.
        #[test]
        fn an_ai_node_in_auto_mode_drives_with_the_round_robin() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let ids = paint.ids();
            let model = ids[0];
            let schema = paint.schemas()[0];
            let base = schema.defaults();
            let mut sim = crate::listeners::Listeners::stand_in();
            let frame = sim.poll(0.5);

            let dir = ai_reels("auto", &[(model, schema, 1.0)]);
            state.reels = Reels::new(&dir);
            let (mut g, gens) = ai_graph("ai-auto", &[model]);
            let ai = g.nodes.iter_mut().find(|n| n.kind == metal_graph::AI_COLOURIST_KIND).unwrap();
            ai.config = metal_graph::Config::from_json_str(r#"{"mode":"auto"}"#).unwrap();
            assert_eq!(ai_mode(ai), AiMode::Auto);

            assert!(ai_offsets(Some(&g), Some(&gens[0]), &mut state, schema, model, &frame, 1.0).is_none());
            let row = row_for_node(
                Some(&g), Some(&gens[0]), &mut *paint, &mut state, 0, &base, Some(&frame), 0.5, None, None,
            );
            assert_eq!(row, drive::row(schema, &base, &frame, &Drive::default()));
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        /// The source's own `modo` (2026-10-08) is the single decision over
        /// the drive: `manual` holds the row at rest; `ai` and `ai_fx` take the
        /// same fitted-colourist path, `ai_fx` adding only the node's embedded
        /// effects (driven by [`embedded_effects`], tested below); an absent key
        /// keeps the pre-`modo` behaviour.
        #[test]
        fn the_source_modo_picks_the_drive() {
            fn set_mode(g: &mut metal_graph::Graph, id: &str, mode: &str) {
                let n = g.nodes.iter_mut().find(|n| n.id == id).unwrap();
                n.config =
                    metal_graph::Config::from_json_str(&format!("{{\"mode\":\"{mode}\"}}")).unwrap();
            }

            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let ids = paint.ids();
            let model = ids[0];
            let schema = paint.schemas()[0];
            let base = schema.defaults();
            let mut sim = crate::listeners::Listeners::stand_in();
            let frame = sim.poll(0.5);

            let dir = ai_reels("mode", &[(model, schema, 1.0)]);
            state.reels = Reels::new(&dir);
            let (mut g, gens) = ai_graph("mode", &[model]);
            let gen = gens[0].clone();

            assert_eq!(node_modo(g.node(&gen)), Modo::Inherit);

            set_mode(&mut g, &gen, "manual");
            assert_eq!(node_modo(g.node(&gen)), Modo::Manual);
            let row = row_for_node(
                Some(&g), Some(&gen), &mut *paint, &mut state, 0, &base, Some(&frame), 0.5, None,
                None,
            );
            assert_eq!(row, drive::row(schema, &base, &Digests::zero(), &Drive::default().at_rest()));

            set_mode(&mut g, &gen, "ai");
            assert_eq!(node_modo(g.node(&gen)), Modo::Ai);
            let ai = row_for_node(
                Some(&g), Some(&gen), &mut *paint, &mut state, 0, &base, Some(&frame), 0.5, None,
                None,
            );
            assert_ne!(
                ai,
                drive::row(schema, &base, &frame, &Drive::of_node(g.node(&gen))),
                "`ai` takes the fitted colourist when one is wired and trained"
            );

            set_mode(&mut g, &gen, "ai_fx");
            assert_eq!(node_modo(g.node(&gen)), Modo::AiFx);
            let ai_fx = row_for_node(
                Some(&g), Some(&gen), &mut *paint, &mut state, 0, &base, Some(&frame), 0.5, None,
                None,
            );
            assert_eq!(ai_fx, ai, "`ai_fx` is `ai` plus embedded effects only");
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        /// Plan §4: `ai_fx` drives the effect floats the gen node carries in
        /// its own config (`config.effects`); `ai` does not. The shape the
        /// properties panel writes — an object of `name -> f32`, or an array of
        /// `{name, value, min, max}` — is accepted either way.
        #[test]
        fn the_embedded_effects_drive_only_in_ai_fx() {
            fn set(g: &mut metal_graph::Graph, id: &str, cfg: &str) {
                let n = g.nodes.iter_mut().find(|n| n.id == id).unwrap();
                n.config = metal_graph::Config::from_json_str(cfg).unwrap();
            }

            use crate::digest::Source as _;

            let mut paint = crate::paint::painter();
            let model = paint.ids()[0];
            let mut sim = crate::digest::Simulated::new(5);
            let frame = sim.poll(3.0);

            let mut g = metal_graph::Graph::new("fx", "fx", metal_graph::Domain::Live);
            let gen = g.add_node("gen", &format!("gen.{model}"), [0.0, 0.0]);

            set(&mut g, &gen, r#"{"mode":"ai_fx","effects":{"grain":0.5,"zoom":0.0}}"#);
            let fx = embedded_effects(g.node(&gen), &frame, &Drive::default());
            assert_eq!(fx.len(), 2, "ai_fx reads the node's embedded effects");
            assert_eq!(fx[0].0, "grain");
            assert_eq!(fx[1].0, "zoom");
            let raw = [0.5f32, 0.0];
            assert!(
                fx.iter().zip(raw).any(|((_, v), r)| (v - r).abs() > 1e-4),
                "ai_fx drives the embedded effects off the digest: {fx:?}"
            );

            set(&mut g, &gen, r#"{"mode":"ai","effects":{"grain":0.5,"zoom":0.0}}"#);
            assert!(
                embedded_effects(g.node(&gen), &frame, &Drive::default()).is_empty(),
                "`ai` carries no embedded effects"
            );

            set(
                &mut g,
                &gen,
                r#"{"mode":"ai_fx","effects":[{"name":"grain","value":0.4,"min":0.0,"max":1.0}]}"#,
            );
            let fx = embedded_effects(g.node(&gen), &frame, &Drive::default());
            assert_eq!(fx.len(), 1, "the array form is accepted too");
            assert_eq!(fx[0].0, "grain");
        }

        #[test]
        fn an_ai_node_fanned_to_two_sources_evaluates_per_schema() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let ids = paint.ids();
            let (m0, m1) = (ids[0], ids[1]);
            let (s0, s1) = (paint.schemas()[0], paint.schemas()[1]);
            let (b0, b1) = (s0.defaults(), s1.defaults());
            let mut sim = crate::listeners::Listeners::stand_in();
            let frame = sim.poll(0.5);

            let dir = ai_reels("two", &[(m0, s0, 1.0), (m1, s1, -1.0)]);
            state.reels = Reels::new(&dir);
            let (g, gens) = ai_graph("ai-two", &[m0, m1]);

            // One node, two schemas, two answers: the offsets are the
            // *model's own* fit, not a single graph-wide evaluation.
            let o0 = ai_offsets(Some(&g), Some(&gens[0]), &mut state, s0, m0, &frame, 1.0).unwrap();
            let o1 = ai_offsets(Some(&g), Some(&gens[1]), &mut state, s1, m1, &frame, 1.0).unwrap();
            assert_eq!(o0[0], 1.0);
            assert_eq!(o1[0], -1.0);

            let r0 = row_for_node(
                Some(&g), Some(&gens[0]), &mut *paint, &mut state, 0, &b0, Some(&frame), 0.5, None, None,
            );
            let r1 = row_for_node(
                Some(&g), Some(&gens[1]), &mut *paint, &mut state, 1, &b1, Some(&frame), 0.5, None, None,
            );
            assert_ne!(r0, drive::row(s0, &b0, &frame, &Drive::default()));
            assert_ne!(r1, drive::row(s1, &b1, &frame, &Drive::default()));
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        #[test]
        fn an_untrained_model_falls_back_to_round_robin_and_says_so() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let ids = paint.ids();
            let model = ids[0];
            let schema = paint.schemas()[0];
            let base = schema.defaults();
            let mut sim = crate::listeners::Listeners::stand_in();
            let frame = sim.poll(0.5);

            // Nothing fitted: an empty reels root.
            let dir = std::env::temp_dir().join(format!("fw-ai-untrained-{}", std::process::id()));
            let _ = metal_vfs::remove_dir_all(&dir);
            state.reels = Reels::new(&dir);
            let (g, gens) = ai_graph("ai-untrained", &[model]);

            assert!(
                ai_offsets(Some(&g), Some(&gens[0]), &mut state, schema, model, &frame, 1.0).is_none(),
                "an untrained model answers no offsets"
            );
            let row = row_for_node(
                Some(&g), Some(&gens[0]), &mut *paint, &mut state, 0, &base, Some(&frame), 0.5, None, None,
            );
            assert_eq!(row, drive::row(schema, &base, &frame, &Drive::default()));
            // `trained() == false` is the fact the node reports; the engine
            // must not have invented a fit.
            assert!(!state.reels.colourist(model, schema).trained());
            let _ = metal_vfs::remove_dir_all(&dir);
        }

        #[test]
        fn a_transmission_without_a_model_node_is_deterministic() {
            let mut paint = crate::paint::painter();
            let mut state = State::new(paint.as_ref());
            let footage = paint.footage();
            let idx = footage.iter().position(|f| *f).expect("a transmission in the manifest");
            let schema = paint.schemas()[idx];
            let base = schema.defaults();
            let mut sim = crate::listeners::Listeners::stand_in();
            let frame = sim.poll(0.5);

            let a = row_for(&mut *paint, &mut state, idx, &base, &frame, 0.5, None, None);
            let b = row_for(&mut *paint, &mut state, idx, &base, &frame, 0.5, None, None);
            assert_eq!(a, b, "a transmission with no ai node is a pure function of the frame");
            assert_eq!(
                a,
                drive::row(schema, &base, &frame, &Drive::default()),
                "without an ai.* node a transmission reacts like any other model"
            );
        }

    /// the loops design notes T10: the musical clock end to end — the real `State::on_air`
    /// fed by the stand-in input (beat 0 is the "1"), and the shipped factory
    /// templates evaluated through `apply_effect_loops` at `music_pos`.
    mod music_loops_e2e {
        use super::super::*;
        use crate::live::Input;
        use crate::paint;

        fn shipped() -> crate::pattern::Library {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let user = std::env::temp_dir().join(format!("fw-e2e-loops-none-{}", std::process::id()));
            let lib = crate::pattern::Library::load(&repo.join("data/loops/templates"), &user);
            assert!(lib.problems.is_empty(), "{:?}", lib.problems);
            lib
        }

        fn stand_in(bpm: f32) -> Input {
            let mut sim = Input::stand_in();
            sim.set_bpm(bpm);
            sim.set_drive(1.0);
            sim
        }

        /// Step 0 exactly at every "1" at 90/128/174, step `k` exactly at each
        /// sixteenth, and no drift after ten simulated minutes at 60 fps.
        #[test]
        fn step_zero_on_every_one_and_no_drift_over_ten_minutes() {
            let paint = paint::painter();
            for bpm in [90.0f32, 128.0, 174.0] {
                let mut state = State::new(paint.as_ref());
                let mut sim = stand_in(bpm);
                assert_eq!(sim.downbeat(), Some(0), "the stand-in's \"1\" is beat 0");
                let sixteenth = 60.0 / bpm as f64 / 4.0;
                let mut last = -1.0f64;
                let frames = 600 * 60;
                for f in 0..=frames {
                    let t = f as f64 / 60.0;
                    state.on_air(&mut sim, t as f32);
                    let m = state.music_pos.beats();
                    assert!(m >= last, "{bpm}: backwards at {t}");
                    last = m;
                    // Expected step from exact arithmetic; the clock rides an
                    // f32 count so allow it to sit on either side of a
                    // boundary only within a microsecond-scale window.
                    let exact = t / sixteenth;
                    let near_edge = (exact - exact.round()).abs() * sixteenth < 1e-3;
                    if !near_edge {
                        assert_eq!(state.music_pos.step as u64, exact.floor() as u64 % 16, "{bpm} BPM, t={t}");
                    }
                }
                // After ten minutes, land on bar instants exactly.
                let bars = (600.0 / (sixteenth * 16.0)).floor() as u64;
                for k in [bars + 1, bars + 2, bars + 3] {
                    let t = k as f64 * sixteenth * 16.0;
                    state.on_air(&mut sim, (t + 1e-3) as f32);
                    assert_eq!(state.music_pos.step, 0, "{bpm} BPM: the \"1\" of bar {k}");
                    assert_eq!(state.music_pos.bar, k, "{bpm} BPM: bar count after 10 min");
                }
                // And at every step of the final bar.
                let t0 = (bars + 4) as f64 * sixteenth * 16.0;
                for i in 0..16u8 {
                    state.on_air(&mut sim, (t0 + (i as f64 + 0.5) * sixteenth) as f32);
                    assert_eq!(state.music_pos.step, i, "{bpm} BPM final bar step {i}");
                }
            }
        }

        /// 124 → 140 mid-set: the position never goes backwards and, once the
        /// new tempo holds, steps sit on the new grid (step 0 on its "1"s).
        #[test]
        fn tempo_change_124_to_140_stays_on_the_grid_and_monotonic() {
            let paint = paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut sim = stand_in(124.0);
            let mut last = -1.0f64;
            let mut t = 0.0f64;
            while t < 60.0 {
                state.on_air(&mut sim, t as f32);
                assert!(state.music_pos.beats() >= last);
                last = state.music_pos.beats();
                t += 1.0 / 60.0;
            }
            sim.set_bpm(140.0);
            while t < 120.0 {
                state.on_air(&mut sim, t as f32);
                assert!(state.music_pos.beats() >= last, "backwards at {t}");
                last = state.music_pos.beats();
                t += 1.0 / 60.0;
            }
            assert_eq!(state.music.bpm(), 140.0);
            let sixteenth: f64 = 60.0 / 140.0 / 4.0;
            let first = (120.0 / (sixteenth * 16.0)).ceil() as u64;
            for k in first..first + 8 {
                let bar_t = k as f64 * sixteenth * 16.0;
                for i in 0..16u8 {
                    let at = bar_t + (i as f64 + 0.5) * sixteenth;
                    state.on_air(&mut sim, at as f32);
                    assert!(state.music_pos.beats() >= last);
                    last = state.music_pos.beats();
                    assert_eq!(state.music_pos.step, i, "140 BPM bar {k} step {i}");
                }
            }
        }

        /// warp-zoom-in-snap + strobe-zoom-in-snap chained on one generator,
        /// clocked by `on_air` at 128 BPM and written by `apply_effect_loops`
        /// (the real per-frame path): the white hit is the warp's snap step.
        #[test]
        fn paired_warp_and_strobe_hit_together_through_the_engine() {
            use metal_graph::Endpoint;
            let lib = shipped();
            let paint = paint::painter();
            let mut state = State::new(paint.as_ref());
            state.loops = lib;
            let mut sim = stand_in(128.0);
            let mut g = metal_graph::Graph::new("e2e", "e2e", metal_graph::Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let warp = g.add_node("warp", "fx.warp", [1.0, 0.0]);
            let flash = g.add_node("flash", "fx.flash", [2.0, 0.0]);
            let out = g.add_node("out", "output.device", [3.0, 0.0]);
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&warp, "image"));
            g.add_edge(Endpoint::new(&warp, "image"), Endpoint::new(&flash, "image"));
            g.add_edge(Endpoint::new(&flash, "image"), Endpoint::new(&out, "image"));
            g.node_mut(&warp).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"fixed":0.5,"loop":"warp-zoom-in-snap"}"#).unwrap();
            g.node_mut(&flash).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"fixed":0.5,"loop":"strobe-zoom-in-snap"}"#).unwrap();
            assert_eq!(effect_loop(&g, &warp, &state.loops).map(|l| l.id.as_str()), Some("warp-zoom-in-snap"));
            assert_eq!(effect_loop(&g, &flash, &state.loops).map(|l| l.id.as_str()), Some("strobe-zoom-in-snap"));
            let num = |g: &metal_graph::Graph, n: &str, k: &str| -> f32 {
                let v: Value = serde_json::from_str(&g.node(n).unwrap().config.to_json_str()).unwrap();
                v[k].as_f64().unwrap_or(f64::NAN) as f32
            };
            let sixteenth: f64 = 60.0 / 128.0 / 4.0;
            // Two bars, sampled mid-step, through the clock.
            for bar in 4..6u64 {
                let mut amount = [0.0f32; 16];
                let mut opacity = [0.0f32; 16];
                for i in 0..16usize {
                    let t = (bar as f64 * 16.0 + i as f64 + 0.5) * sixteenth;
                    state.on_air(&mut sim, t as f32);
                    assert_eq!(state.music_pos.step as usize, i);
                    let mut frame = g.clone();
                    let mut rows = crate::bind::Rows::default();
                    apply_effect_loops(&mut frame, &mut rows, &state.loops, state.music_pos);
                    amount[i] = num(&frame, &warp, "amount");
                    opacity[i] = num(&frame, &flash, "opacity");
                }
                assert!(amount.iter().all(|a| a.is_finite()) && opacity.iter().all(|o| o.is_finite()), "{amount:?} {opacity:?}");
                let snap = (0..16)
                    .max_by(|&a, &b| {
                        let j = |i: usize| (amount[i] - amount[(i + 15) % 16]).abs();
                        j(a).total_cmp(&j(b))
                    })
                    .unwrap();
                let hit = (0..16).max_by(|&a, &b| opacity[a].total_cmp(&opacity[b])).unwrap();
                assert_eq!(hit, snap, "strobe on {hit}, warp snaps on {snap}: {amount:?} {opacity:?}");
                assert_eq!(opacity.iter().filter(|&&o| o >= 0.5).count(), 1, "{opacity:?}");
            }
        }

        /// The dembow flash, clocked by `on_air` at 95 BPM through
        /// `apply_effect_loops`: its opacity peaks on steps {3,4,11,12}.
        #[test]
        fn dembow_hits_on_3_4_11_12_through_the_engine() {
            use metal_graph::Endpoint;
            let paint = paint::painter();
            let mut state = State::new(paint.as_ref());
            state.loops = shipped();
            let mut sim = stand_in(95.0);
            let mut g = metal_graph::Graph::new("e2e", "e2e", metal_graph::Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let flash = g.add_node("flash", "fx.flash", [1.0, 0.0]);
            let out = g.add_node("out", "output.device", [2.0, 0.0]);
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&flash, "image"));
            g.add_edge(Endpoint::new(&flash, "image"), Endpoint::new(&out, "image"));
            g.node_mut(&flash).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"fixed":0.5,"loop":"dembow-flash"}"#).unwrap();
            let l = effect_loop(&g, &flash, &state.loops).expect("dembow-flash plays");
            let crate::pattern::TrackTarget::Float { param } = l.tracks[0].target.clone() else {
                panic!("dembow lead is a float track")
            };
            let sixteenth: f64 = 60.0 / 95.0 / 4.0;
            let mut v = [0.0f32; 16];
            for i in 0..16usize {
                let t = (32.0 + i as f64 + 0.5) * sixteenth;
                state.on_air(&mut sim, t as f32);
                let mut frame = g.clone();
                let mut rows = crate::bind::Rows::default();
                apply_effect_loops(&mut frame, &mut rows, &state.loops, state.music_pos);
                let cfg: Value = serde_json::from_str(&frame.node(&flash).unwrap().config.to_json_str()).unwrap();
                v[i] = cfg[param.as_str()].as_f64().unwrap() as f32;
            }
            let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
            let hits: Vec<usize> = (0..16).filter(|&i| v[i] > (lo + hi) / 2.0).collect();
            assert_eq!(hits, vec![3, 4, 11, 12], "{v:?}");
        }

        /// the loops design notes T15: a playing song drives the clock from its sidecar
        /// (not the stand-in's 128 BPM), a seek jumps back, stop hands back.
        #[test]
        fn a_playing_song_follows_its_tempo_sidecar_and_seeks() {
            let paint = paint::painter();
            let mut state = State::new(paint.as_ref());
            let mut sim = stand_in(128.0);
            // 100 BPM, first beat at 0.2 s, the "1" on beat 1.
            let beats: Vec<f64> = (0..300).map(|i| 0.2 + i as f64 * 0.6).collect();
            let downbeats: Vec<f64> = beats.iter().skip(1).step_by(4).copied().collect();
            let track = crate::clock::TempoTrack { segments: vec![(0.0, 100.0)], beats, downbeats: downbeats.clone() };
            // Engine time 50 s = song second 0 (the anchor).
            state.play_song(track.clone(), 0.0);
            state.on_air(&mut sim, 50.0);
            for (k, one) in downbeats.iter().take(30).enumerate() {
                // A millisecond in: engine time is f32, so 50 + t rounds.
                state.on_air(&mut sim, 50.0 + *one as f32 + 1e-3);
                assert_eq!((state.music_pos.step, state.music_pos.bar), (0, k as u64), "\"1\" {k}");
            }
            assert_eq!(state.music.bpm(), 100.0);
            // Seek back to bar 2's "1" + one beat.
            state.play_song(track, downbeats[2] + 0.6);
            state.on_air(&mut sim, 200.0);
            assert_eq!((state.music_pos.bar, state.music_pos.step), (2, 4));
            // Stop: the input takes over, never backwards from there.
            let at = state.music_pos.beats();
            state.stop_song();
            state.on_air(&mut sim, 200.1);
            assert!(state.music_pos.beats() >= at);
            assert!(state.song.is_none());
        }
    }
}

/// Plan "Audio input is a node", Design 9: what a source reads.
#[cfg(test)]
mod audio_node_tests {
    use super::*;
    use crate::listeners::{Digests, Listeners, SourceKey};

    /// A pool that opens every source as a stand-in at its own tempo, so two
    /// sources draw two different rows at the same instant.
    fn pool() -> Listeners {
        Listeners::with_opener(
            SourceKey::Default,
            Box::new(|k: &SourceKey| {
                let mut i = Input::stand_in();
                i.set_bpm(90.0 + 11.0 * k.as_pick().len() as f32);
                Ok(i)
            }),
        )
    }

    fn audio(g: &mut metal_graph::Graph, source: &str) -> String {
        let id = g.add_node("audio", crate::listeners::AUDIO_LIVE, [0.0, 0.0]);
        g.node_mut(&id).unwrap().config =
            metal_graph::Config::from_json_str(&serde_json::json!({ "source": source }).to_string()).unwrap();
        id
    }

    #[test]
    fn a_source_reads_the_audio_node_wired_to_its_control() {
        let mut paint = crate::paint::painter();
        let mut state = State::new(paint.as_ref());
        let (model, schema) = (paint.ids()[0], paint.schemas()[0]);
        let base = schema.defaults();
        let mut g = metal_graph::Graph::new("wired", "wired", metal_graph::Domain::Live);
        let a = audio(&mut g, "mic-a");
        let b = audio(&mut g, "mic-bbbb");
        let gen = g.add_node("gen", &format!("gen.{model}"), [0.0, 0.0]);
        // No factory loop over the floats: the row is the audio drive alone.
        g.node_mut(&gen).unwrap().config = metal_graph::Config::from_json_str(r#"{"loop":"none"}"#).unwrap();
        g.add_edge(metal_graph::Endpoint::new(&b, "control"), metal_graph::Endpoint::new(&gen, "control"));

        let mut pool = pool();
        let t = 3.3;
        let clock = pool.clock_frame(&mut state.looper, t);
        let d = pool.digests(&g, clock, t);
        let (fa, fb) = (*d.node(&a).unwrap(), *d.node(&b).unwrap());
        assert_ne!(fa.raw, fb.raw, "two sources, two digests");
        assert_eq!(d.wired(&g, &gen).unwrap().raw, fb.raw, "the source reads the node wired to it");

        let row = row_for_node(Some(&g), Some(&gen), &mut *paint, &mut state, 0, &base, d.wired(&g, &gen), t, None, None);
        assert_eq!(row, drive::row(schema, &base, &fb, &Drive::default()));
        assert_ne!(row, drive::row(schema, &base, &fa, &Drive::default()), "not the other audio node");

        // The dials are the source's own: a node config with depth 0 is the
        // base row, whatever the audio does.
        g.node_mut(&gen).unwrap().config =
            metal_graph::Config::from_json_str(r#"{"loop":"none","drive":{"depth":0.0,"colour":false}}"#).unwrap();
        let rest = row_for_node(Some(&g), Some(&gen), &mut *paint, &mut state, 0, &base, Some(&fb), t, None, None);
        assert_eq!(rest, drive::row(schema, &base, &Digests::zero(), &Drive::default().at_rest()));
    }

    #[test]
    fn a_source_with_nothing_wired_is_static_and_says_so() {
        let mut paint = crate::paint::painter();
        let mut state = State::new(paint.as_ref());
        let (model, schema) = (paint.ids()[0], paint.schemas()[0]);
        let base = schema.defaults();
        let mut g = metal_graph::Graph::new("static", "static", metal_graph::Domain::Live);
        audio(&mut g, "mic-a");
        let gen = g.add_node("gen", &format!("gen.{model}"), [0.0, 0.0]);
        // No factory loop over the floats: the row is the audio drive alone.
        g.node_mut(&gen).unwrap().config = metal_graph::Config::from_json_str(r#"{"loop":"none"}"#).unwrap();
        let mut pool = pool();
        let mut rows = Vec::new();
        for t in [0.5f32, 7.25, 31.0] {
            let clock = pool.clock_frame(&mut state.looper, t);
            let d = pool.digests(&g, clock, t);
            // No fallback to the graph's (only) audio node: nothing is wired.
            assert!(d.wired(&g, &gen).is_none(), "an unwired source has no digest");
            rows.push(row_for_node(Some(&g), Some(&gen), &mut *paint, &mut state, 0, &base, None, t, None, None));
        }
        assert!(rows.windows(2).all(|w| w[0] == w[1]), "static: the same row at every instant");
        assert_eq!(rows[0], drive::row(schema, &base, &Digests::zero(), &Drive::default().at_rest()), "the base row");
    }

    // -- H3: cam.track writes the reserved camera floats -------------------

    fn h3_manifest() -> metal_schema::Manifest {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        metal_schema::Manifest::parse(&metal_vfs::read_to_string(path).unwrap()).unwrap()
    }

    /// `capture → sensor.yolo → cam.track → gen.turing`, with turing's row and
    /// the reserved `cam_x/cam_y/cam_z` positions found by name in the manifest.
    fn h3_fixture() -> (metal_graph::Graph, metal_graph::NodeId, metal_graph::NodeId, metal_schema::Manifest, [usize; 3], Vec<f32>) {
        use metal_graph::{Domain, Endpoint, Graph};
        let paint = crate::paint::painter();
        let ids = paint.ids();
        let ti = ids.iter().position(|id| *id == "turing").expect("turing is in the suite");
        let base = paint.schemas()[ti].defaults();

        let manifest = h3_manifest();
        let model = manifest.models.iter().find(|m| m.id == "turing").expect("turing in the manifest");
        assert!(model.camera_track, "turing declares camera_track");
        let idx = camera_track_indices(model).expect("turing declares cam_x/y/z");

        let mut g = Graph::new("h3", "h3", Domain::Live);
        let capture = g.add_node("cap", "capture", [0.0, 0.0]);
        let sensor = g.add_node("sensor", "sensor.yolo", [1.0, 0.0]);
        let cam = g.add_node("cam", "cam.track", [2.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&capture, "image"), Endpoint::new(&sensor, "image"));
        g.add_edge(Endpoint::new(&sensor, "control"), Endpoint::new(&cam, "control"));
        g.node_mut(&cam).unwrap().config = metal_graph::camera_track::CamTrackConfig {
            targets: vec![gen.clone()],
            strength: 1.0,
            invert: [false; 3],
        }
        .to_config();
        (g, gen, sensor, manifest, idx, base)
    }

    /// The eyes reading the `sensor.yolo` node found this frame.
    fn sensor_readings(sensor: &metal_graph::NodeId, x: f32) -> HashMap<metal_graph::NodeId, metal_sense::Control> {
        let c = metal_sense::Control::new([("eye_x", x), ("eye_y", 0.5), ("eye_z_est", 0.5)]);
        [(sensor.clone(), c)].into_iter().collect()
    }

    /// The required H3 test: a stand-in head reading moves the target
    /// generator's reserved `cam_x` float.
    #[test]
    fn a_stand_in_head_moves_the_targets_camera() {
        let (g, gen, sensor, manifest, idx, base) = h3_fixture();

        // A face right of centre: cam_x moves positive.
        let mut rows: HashMap<metal_graph::NodeId, Vec<f32>> = [(gen.clone(), base.clone())].into_iter().collect();
        let moved = apply_camera_track(&g, &mut rows, &manifest, &sensor_readings(&sensor, 0.9));
        assert_eq!(moved, 1, "one target row moved");
        let want = metal_sense::eyes_to_camera_offset(
            &metal_sense::Control::new([("eye_x", 0.9), ("eye_y", 0.5), ("eye_z_est", 0.5)]),
            metal_sense::DEFAULT_EYE_SENSITIVITY,
        );
        assert!((rows[&gen][idx[0]] - want[0]).abs() < 1e-6, "cam_x = {}", rows[&gen][idx[0]]);
        assert!(rows[&gen][idx[0]] > 0.0);

        // A face left of centre moves it the other way.
        let mut rows2: HashMap<metal_graph::NodeId, Vec<f32>> = [(gen.clone(), base)].into_iter().collect();
        apply_camera_track(&g, &mut rows2, &manifest, &sensor_readings(&sensor, 0.1));
        assert!(rows2[&gen][idx[0]] < 0.0, "a left reading pans negative-x");

        // No reading for the sensor: nothing moves.
        let mut rows3: HashMap<metal_graph::NodeId, Vec<f32>> = [(gen.clone(), g_defaults(&g, &manifest, &gen))].into_iter().collect();
        let before = rows3[&gen].clone();
        assert_eq!(apply_camera_track(&g, &mut rows3, &manifest, &HashMap::new()), 0);
        assert_eq!(rows3[&gen], before);
    }

    fn g_defaults(_g: &metal_graph::Graph, _m: &metal_schema::Manifest, _n: &metal_graph::NodeId) -> Vec<f32> {
        let paint = crate::paint::painter();
        let ids = paint.ids();
        paint.schemas()[ids.iter().position(|id| *id == "turing").unwrap()].defaults()
    }

    /// The required H3 test: the head offset is summed on top of a camera the
    /// sequence (or an `ai.*` node) already set, never replacing it.
    #[test]
    fn head_offset_adds_to_the_sequence_camera() {
        let (g, gen, sensor, manifest, idx, base) = h3_fixture();
        let mut base = base;
        base[idx[0]] = 2.0; // the sequence put the camera here
        base[idx[2]] = -1.5;
        let mut rows: HashMap<metal_graph::NodeId, Vec<f32>> = [(gen.clone(), base.clone())].into_iter().collect();
        apply_camera_track(&g, &mut rows, &manifest, &sensor_readings(&sensor, 0.75));
        let off = metal_sense::eyes_to_camera_offset(
            &metal_sense::Control::new([("eye_x", 0.75), ("eye_y", 0.5), ("eye_z_est", 0.5)]),
            metal_sense::DEFAULT_EYE_SENSITIVITY,
        );
        assert!((rows[&gen][idx[0]] - (2.0 + off[0])).abs() < 1e-6, "summed, not replaced");
        assert!((rows[&gen][idx[2]] - (-1.5 + off[2])).abs() < 1e-6, "cam_z summed too");
    }

    /// H3: an empty `targets` means every generator in the graph whose model
    /// declares `camera_track()` — and only those.
    #[test]
    fn an_empty_target_list_tracks_every_camera_track_generator() {
        use metal_graph::{Domain, Endpoint, Graph};
        let (_, _, sensor, manifest, idx, base) = h3_fixture();
        let mut g = Graph::new("h3-all", "h3-all", Domain::Live);
        let capture = g.add_node("cap", "capture", [0.0, 0.0]);
        let sensor = {
            let s = g.add_node("sensor", "sensor.yolo", [1.0, 0.0]);
            g.add_edge(Endpoint::new(&capture, "image"), Endpoint::new(&s, "image"));
            s
        };
        let cam = g.add_node("cam", "cam.track", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&sensor, "control"), Endpoint::new(&cam, "control"));
        // The config is left empty on purpose.
        g.node_mut(&cam).unwrap().config = metal_graph::camera_track::CamTrackConfig::default().to_config();
        let a = g.add_node("a", "gen.turing", [3.0, 0.0]);
        let b = g.add_node("b", "gen.turing", [3.0, 1.0]);
        let plain = g.add_node("plain", "gen.ascii", [3.0, 2.0]);

        let mut rows: HashMap<metal_graph::NodeId, Vec<f32>> = [
            (a.clone(), base.clone()),
            (b.clone(), base.clone()),
            (plain.clone(), vec![0.0f32; 40]),
        ]
        .into_iter()
        .collect();
        let moved = apply_camera_track(&g, &mut rows, &manifest, &sensor_readings(&sensor, 0.9));
        assert_eq!(moved, 2, "both camera-track generators, not the ascii one");
        assert!(rows[&a][idx[0]] > 0.0 && rows[&b][idx[0]] > 0.0);
        assert_eq!(rows[&plain], vec![0.0f32; 40], "a model with no camera is untouched");
    }

    /// The required H3-finish test: a frame captured upstream of a
    /// `sensor.yolo` node is fed to that node's detector, whose reading moves
    /// the target generator's reserved camera floats. With no frame the
    /// detector never runs and the camera stays put — the frame is what gates
    /// the reading.
    #[test]
    fn a_captured_frame_feeds_the_detector_and_moves_the_camera() {
        let (g, gen, sensor, manifest, idx, base) = h3_fixture();
        let capture = g
            .nodes
            .iter()
            .find(|n| n.kind == "capture")
            .expect("the fixture wires a capture node")
            .id
            .clone();
        let script = metal_sense::Control::new([("eye_x", 0.9), ("eye_y", 0.5), ("eye_z_est", 0.5)]);
        let pixel = vec![0u8; (HEAD_DETECT_W * HEAD_DETECT_H * 4) as usize];

        let mut paint = crate::paint::painter();
        let mut state = State::new(paint.as_ref());
        state.head_detectors.insert(
            sensor.clone(),
            Box::new(metal_sense::StandInDetector::fixed(metal_sense::Mode::Eyes, script)),
        );
        state.head_frames.insert(capture, metal_sense::Image::new(HEAD_DETECT_W, HEAD_DETECT_H, pixel));

        assert_eq!(
            poll_head_sensors(&g, &mut state, HEAD_DETECT_W, HEAD_DETECT_H),
            1,
            "one sensor ran"
        );
        assert!(state.head.contains_key(&sensor), "the reading is stored under the sensor node");

        let mut rows: HashMap<metal_graph::NodeId, Vec<f32>> = [(gen.clone(), base.clone())].into_iter().collect();
        assert_eq!(apply_camera_track(&g, &mut rows, &manifest, &state.head), 1);
        assert!(
            rows[&gen][idx[0]] > 0.0,
            "a face right of centre pans the camera positive-x, got {}",
            rows[&gen][idx[0]]
        );

        // No frame this frame: the detector is not even called, so no reading
        // is invented and the camera does not move.
        let mut no_frame = State::new(paint.as_ref());
        no_frame.head_detectors.insert(
            sensor.clone(),
            Box::new(metal_sense::StandInDetector::fixed(
                metal_sense::Mode::Eyes,
                metal_sense::Control::new([("eye_x", 0.9), ("eye_y", 0.5), ("eye_z_est", 0.5)]),
            )),
        );
        assert_eq!(poll_head_sensors(&g, &mut no_frame, HEAD_DETECT_W, HEAD_DETECT_H), 1);
        assert!(no_frame.head.is_empty(), "with no captured frame there is no reading to store");
        let mut rows2: HashMap<metal_graph::NodeId, Vec<f32>> = [(gen.clone(), base)].into_iter().collect();
        let before = rows2[&gen].clone();
        assert_eq!(apply_camera_track(&g, &mut rows2, &manifest, &no_frame.head), 0);
        assert_eq!(rows2[&gen], before, "no frame, no offset");
    }
}

/// G1: the GPU backend's frame is the CPU backend's frame.
#[cfg(all(test, feature = "gpu"))]
mod gpu_centric_tests {
    use super::*;
    use crate::backend::Backend;

    fn painter(b: Backend) -> Box<dyn Paint> {
        Box::new(crate::paint::Gpu::with_backend(metal_render::headless::Renderer::new(), b))
    }

    /// Three overlapping layers (model, opacity, blend all different) on one
    /// device, rendered once per backend through `render_device`.
    fn frame(b: Backend, w: u32, h: u32) -> (Vec<u8>, bool) {
        let mut paint = painter(b);
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);
        let device = format!("g1-parity-{}-{}", b.as_str(), std::process::id());
        let mut layers = Vec::new();
        for (i, (blend, opacity)) in
            [(Blend::Normal, 1.0), (Blend::Add, 0.6), (Blend::Screen, 0.8)].into_iter().enumerate()
        {
            let mut l = Layer::new(&format!("g1-{i}"), "l", i);
            l.outputs = vec![device.clone()];
            l.blend = blend;
            l.opacity = opacity;
            layers.push(l);
        }
        state.layers = layers;
        state.sources.insert(device.clone(), layers::Device::default());
        let on_gpu = matches!(
            render_device_frame(&mut *paint, &mut sim, &mut state, &device, w, h, &mut |_| None),
            DeviceFrame::Gpu(_)
        );
        let (px, ..) = render_device(&mut *paint, &mut sim, &mut state, &device, w, h);
        (px, on_gpu)
    }

    #[test]
    fn the_gpu_backend_frame_matches_the_cpu_backend_and_never_left_the_gpu() {
        let (cpu, cpu_gpu) = frame(Backend::Cpu, 96, 54);
        let (gpu, gpu_gpu) = frame(Backend::Gpu, 96, 54);
        assert!(!cpu_gpu, "the Cpu backend hands back bytes");
        assert!(gpu_gpu, "the Gpu backend hands back a texture when nothing needs CPU pixels");
        assert_eq!(cpu.len(), gpu.len());
        let sum: u64 = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b) as u64).sum();
        let mean = sum as f64 / cpu.len() as f64;
        let max = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(mean < 0.6 && max <= 3, "mean {mean:.3} max {max}");
        assert!(cpu.chunks_exact(4).any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0), "a real picture");
    }

    /// G2: a layer showing only its `span` window of a source stays a texture
    /// (a GPU rectangle copy) and matches the CPU crop.
    #[test]
    fn a_span_crop_stays_on_the_gpu_and_matches_the_cpu() {
        let one = |b: Backend| {
            let mut paint = painter(b);
            let mut sim = crate::listeners::Listeners::stand_in();
            let mut state = State::new(paint.as_ref());
            state.paused_at = Some(3.0);
            let device = format!("g2-span-{}-{}", b.as_str(), std::process::id());
            let other = format!("g2-span-other-{}", std::process::id());
            let mut l = Layer::new("g2-span", "l", 1);
            l.outputs = vec![device.clone(), other.clone()];
            l.span.insert(device.clone(), Sector { x: 0.0, y: 0.0, w: 0.5, h: 1.0 });
            l.span.insert(other.clone(), Sector { x: 0.5, y: 0.0, w: 0.5, h: 1.0 });
            state.layers = vec![l];
            state.sources.insert(device.clone(), layers::Device::default());
            state.sources.insert(other, layers::Device::default());
            let on_gpu = matches!(
                render_device_frame(&mut *paint, &mut sim, &mut state, &device, 96, 54, &mut |_| None),
                DeviceFrame::Gpu(_)
            );
            (render_device(&mut *paint, &mut sim, &mut state, &device, 96, 54).0, on_gpu)
        };
        let (cpu, _) = one(Backend::Cpu);
        let (gpu, on_gpu) = one(Backend::Gpu);
        assert!(on_gpu, "a span crop is no reason to read back");
        let mean = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b) as f64).sum::<f64>() / cpu.len() as f64;
        assert!(mean < 0.6, "mean {mean:.3}");
        assert!(cpu.chunks_exact(4).any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0));
    }

    /// G2: `still` and `capture` pictures are uploaded once and re-uploaded only
    /// when their version changes.
    #[test]
    fn cached_source_textures_upload_only_on_change() {
        let mut paint = painter(Backend::Gpu);
        let mut makes = 0;
        for v in [1u64, 1, 1, 2, 2] {
            let t = paint.cached_texture("k", v, &mut || {
                makes += 1;
                Some((vec![v as u8; 8 * 4 * 4], 8, 4))
            });
            assert!(t.is_some());
        }
        assert_eq!(makes, 2);
    }
}
