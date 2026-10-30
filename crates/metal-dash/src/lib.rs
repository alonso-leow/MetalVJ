//! MetalVJ — the app.
//!
//! The operator's face, the one the handoff calls the Dashboard: *"simple:
//! change between models and hit play. Used by the venue operator. Not an
//! editable timeline."* The studio (`fw-studio`) is where an animation is
//! authored; this is where one is run.
//!
//! # The page is a list of sources — 2026-08-24
//!
//! It used to be a list of devices, each holding sources. That is the
//! technically true shape (a device composes a stack; the compositor walks it),
//! and it is the wrong thing to put in front of somebody who is running a room.
//! *"It is better to base the list on the sources, and within each determine
//! where to send it (the device and the mask); that keeps it centred on the
//! artistic side and not the technical one."*
//!
//! So **BROADCAST** is a grid of sources. One card per source, and on the card
//! is everything about it: what it draws, where it goes, what shape it is when
//! it gets there, its own picture and its own floats. Nothing underneath
//! changed — a source is still a [`Layer`] on a device and the compositor still
//! walks devices — the page inverts the list, which is a rendering decision and
//! not a data one.
//!
//! What that cost on this side is three things:
//!
//! - Every source decides its own content ([`layers::Feed`]), so the global
//!   router mode is no longer a control. The router still runs, over one digest;
//!   sources subscribe to it. [`layers::Airing`] is the whole of what they can
//!   see.
//! - A source can bind to a contour ([`Layer::mask`]), so two sources on one
//!   projector can light two real objects.
//! - Each source is its own picture (`/source.jpg`) and its own float row
//!   (`/monitor`), because a card that shows neither is a row in a table with
//!   more space around it.
//!
//! The **MONITOR** is neither chrome nor decoration: the sixteen digest columns
//! run across the header, and the actual float row reaching each source is on
//! that source's card, named. Without it "the visual is not reacting" is
//! unanswerable — you cannot tell a dead input from a mapping that is not moving
//! from a model ignoring the floats it was given. With it, you look.
//!
//! **What is simulated and what is real.** The renderers are real: these are
//! the same wgpu models the studio previews, drawn headlessly at whatever size
//! the page asks for. The *digest* is the stand-in — `crate::digest::Simulated`
//! stands where microphone and loopback capture will go, producing the same
//! sixteen columns `docs/AUDIO_FEATURES.md` fixes. When the real digester
//! lands it implements `digest::Source` and one struct is deleted.
//!
//! ```text
//! metal-dash --port 7900
//! ```
use std::net::{TcpListener, TcpStream};
// S.U7b: `Shared` moved behind `Arc<Mutex<Shared>>` so the listener thread
// and (on a `gpu` build) the winit thread can both reach it — see `Shared`'s
// own doc comment. `Author::state`'s own `Mutex` (`gpu`-only) is unrelated
// and predates this.
use clap::Parser;
use metal_web::{assets, Request};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
// Conjunto C: `fw-studio` absorbed whole, under `/author/*`. See
// `author::mod` for why it is gated on `gpu` and how the prefix works.
#[cfg(feature = "gpu")]
mod author;
// O.12.b′ / S.U4a: the call layer. `serve`'s `match` used to hold every
// handler's body directly; now every arm is a typed function in here, and
// `serve` (below) is a thin wrapper that walks `call::ROUTES`. Read its
// module doc before adding a route: it is also the door the native shell
// (`S.U5` onward) will call through, so a route only in `serve`'s own logic
// is exactly the second path rule 6 forbids.
mod call;
// S.X: the chat — one `data/chats/<id>.json` conversation each, plus the
// worker thread that drives `metal_agent::agent_loop::run_turn` against the
// provider `/ai-config` writes. Not gated on `gpu`: a no-GPU host can still
// take a chat turn's filesystem tools, exactly as the sidebar's controls do.
#[cfg(feature = "agent")]
mod chat;
// The engine half — `resolve → compile → execute → output`, the app's own
// `State`, the graph files and the module set it owns (layers, compose,
// output, overlay, sequence, drive, digest, router, reel, live, escena,
// devices, trigger, atomic) — lives in `metal-perform` since LAY.9b. `metal-dash`
// keeps the routes and the window; these re-exports are what let every
// remaining `crate::layers::…`/`crate::output::…` path here and in `call/`
// keep reading the way it always did, with one definition on the other side.
#[cfg(test)]
pub(crate) use metal_perform::drive;
pub(crate) use metal_perform::{
    atomic, capture, devices, digest, escena, fixture, layers, listeners, live, node_layer, output,
    overlay, reel, router, sequence, switch, trigger, venue,
};
// The engine's own entry points, same reason: `call/`, `shell/` and the
// tests below all reach them as `crate::…`, exactly as they did when this
// file owned them.
pub(crate) use metal_perform::engine::*;
// `S.U16`: the monitor mode — a winit+softbuffer window that paints what
// `GET /frame` serves. Same gate as the `winit` dependency itself. It also
// names `call::author` (gpu only), so `desktop` + `glow` without `gpu` still
// fails here, as it did before this gate.
#[cfg(all(feature = "desktop", any(feature = "gpu", feature = "glow")))]
mod monitor;
mod qoi;
#[cfg(all(feature = "desktop", any(feature = "gpu", feature = "glow")))]
mod remote_layers;
#[cfg(test)]
mod test_fixtures;

// The lanes playhead as the shell drew it, read by `GET /author/play`.
mod playhead;
// P-D: the `clock.timecode` readers and `GET /timecode`, `GET /artnet/nodes`.
mod json_memo;
mod paint;
mod warm;
mod perf;
mod timecode;
// The in-app updater's settings and wire replies (`call/update.rs` is the two
// routes). No git: GitHub Releases in, a verified binary out. Gated on
// `desktop` — Android installs an APK, not a file this process can rename — so
// a build without it answers `GET /update` with `state: "off"`.
mod update;
// The shell-mode wire vocabulary, shared by `call::settings` (which must
// validate it on every build) and the shell (which consumes it). Top-level
// so neither has to reach into the other; see the module note.
mod frame_cap;
mod ui_scale;
mod shell_mode;
// O.10b: external input readers (`src/input/`) — the pieces every reader
// shares (learn arm, loopback POST) and the OSC reader. Each reader ends in
// `POST /trigger/<id>` over loopback, the same route a human uses (rule 6).
// O.10b: OSC built; MIDI built too, behind `--features midi` (`midir` links
// the system ALSA sequencer, so the default build must not need ALSA headers
// — see `crates/metal-dash/Cargo.toml`).
mod input;
// The control surface (grid layout, MIDI bindings, freeze/rec/loops, fn
// pages): pure, so `call::settings` validates it and the shell paints it.
pub mod surface;
// O.12 / S.U5: the native egui shell (dash.html/page/dash.html were deleted
// 15-sept-2026, S.U18 — this window is the only client).
// The three layers (`shell`, `components`, `classes`), the gates that keep
// them apart, and — `S.U6`/`S.U15` — `shell::window`, the winit platform
// layer that opens the window `main` runs on this thread while the listener
// serves from another, over either a `wgpu` or an `egui_glow` painter.
// Behind `any(gpu, glow)` whole: a build with neither has no window to open
// and serves exactly as it always did.
#[cfg(any(feature = "gpu", feature = "glow", target_arch = "wasm32"))]
mod shell;
#[cfg(test)]
mod gallery_gen;
// Screenshots of the shell's states — outside `shell/` because it names
// kittest's wgpu renderer (gate 4). Test-only, `-F shots`.
#[cfg(all(test, feature = "shots"))]
mod shots;
use layers::Collections;
#[cfg(test)]
use layers::{Blend, ClipMode, Composition, Layer, Limits, Sector};
use listeners::Listeners;
use output::Outputs;
use reel::Reels;
// Only reached from `mod tests` below now — every non-test use moved to
// `call::{outputs, transmit, render}` along with the handler bodies that
// used to call them from this file's own `match`.
#[cfg(test)]
use layers::Collection;
#[cfg(test)]
use output::Output;
// The engine state and graph/AI helpers (moved here from the root of this file
// so the browser build can see them): `Shared`, `Author`, `list_graphs`,
// `node_catalog` and the reply builders. See `app.rs`.
mod app;
pub use app::*;
// The stores and the preview trait: `FsStore` native, `LocalStore` in the
// browser. See `store.rs`.
mod i18n;
pub mod preview;
pub mod store;
#[cfg(target_arch = "wasm32")]
mod web;

#[derive(Parser)]
#[command(
    name = "metal-dash",
    about = "MetalVJ — run an animation against music"
)]
struct Cli {
    #[arg(long, default_value_t = 7900)]
    port: u16,
    /// O.10b: the UDP port the OSC reader listens on. The reader binds it,
    /// decodes each datagram's address, and fires the matching trigger over
    /// loopback HTTP. The default is the OSC convention (9000).
    #[arg(long, default_value_t = 9000)]
    osc_port: u16,
    /// O.10b: only connect MIDI input ports whose ALSA name contains this
    /// substring. Without it the reader connects every input port it can see.
    /// Only used by a build with `--features midi`; the flag parses in every
    /// build so a script does not have to know which one it is talking to.
    #[arg(long)]
    midi_port: Option<String>,
    /// What to bind to. Loopback by default and deliberately: this app has no
    /// authentication of any kind and every route changes what a room is
    /// looking at. Anything else is for putting it behind a reverse proxy that
    /// does have some — see `docs/OVERLAYS.md` on the review surface.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Where `outputs.json` and `collections.json` live. Defaults to the
    /// repo's `data/`. Set it on a server, where the build tree is a checkout
    /// and the mapping is state.
    #[arg(long)]
    data: Option<std::path::PathBuf>,
    /// Listen to a real input instead of the stand-in.
    ///
    /// Takes a source name as `pactl list short sources` prints it — in a booth
    /// that is the one ending in `.monitor`, which is the mix bus rather than a
    /// microphone pointed at the room. Bare `--listen` takes the default input,
    /// which on a laptop is usually the built-in mic.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    listen: Option<String>,
    /// `S.U16`: run this instance as the monitor — a fullscreen `winit`
    /// window that paints what `GET /frame` returns and nothing else (the
    /// slaving rule, `O.8`: it does not render). The device id is optional:
    /// a bare `--monitor` asks the master's own `/monitor` route which
    /// device is on the air. Interim transport, same-machine loopback —
    /// see `src/monitor.rs`'s module doc for the `S.R` note and the flag's
    /// removal clause.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    monitor: Option<String>,
    /// Which monitor `--monitor` fills: a connector name as the compositor
    /// reports it (`HDMI-A-1`, `DP-2`, `eDP-1`). Without it the monitor
    /// named like the device wins, else the first external one.
    #[arg(long)]
    display: Option<String>,
    /// Do not open the native window — serve the dashboard headless, on
    /// this thread, exactly as a build without the `gpu` feature does.
    ///
    /// For the JS harnesses and a CI box with an adapter but no compositor.
    /// On a build without `gpu` the flag is accepted and changes nothing,
    /// so one command line works against both builds.
    #[arg(long)]
    no_window: bool,
    /// Force the paint backend (`wgpu` or `glow`) instead of letting
    /// `metal_render::gpu::probe` choose. `S.U15`'s verification seam, not the
    /// real chooser — the probe is, and stays the default when this is
    /// absent. Ignored on a build with only one of the two backends
    /// compiled in (that build has nothing to choose between); an
    /// unrecognised value is a startup error, same as a bad `--port`.
    /// `FW_BACKEND` is the same knob as an env var, for the cases (a
    /// screenshot script, a systemd unit) where a flag is awkward to add;
    /// the flag wins when both are set.
    #[arg(long)]
    backend: Option<String>,
    /// Which house style the shell opens in: `dark` (the phosphor tube
    /// `web/theme.css` paints, after dark) or `light` (the same universe
    /// with the tube switched on — `Theme::daylight`, `plan-opus.md`
    /// § O.17). The alias of each is the other theme's own name
    /// (`phosphor`/`daylight`). `FW_THEME` is the same knob as an env var,
    /// in the same cases `FW_BACKEND` is; the flag wins when both are set.
    /// The emphasis is common to both styles, so switching is changing the
    /// ground, not the signal.
    #[cfg_attr(not(any(feature = "gpu", feature = "glow")), allow(dead_code))]
    #[arg(long)]
    theme: Option<String>,
    /// This machine's name for `Output.node`/`composed_on` (S.R): default the hostname.
    #[arg(long)]
    node: Option<String>,
    /// Which release channel the in-app updater watches: `main` (`v1.2.3`
    /// tags), a branch name such as `dev` (`dev-v1.2.4`), or `off` (never
    /// check, never apply). Overrides `[update].channel` in
    /// `~/.config/metalvj/config.toml` for this run only; see
    /// `crates/metal-update`'s crate docs for the tag convention. No flag
    /// leaves the config file in charge, whose default is `main`.
    ///
    /// The update itself is never automatic: `GET /update` only reports, and
    /// `POST /update {"op":"apply","tag":"..."}` is the one call that downloads.
    #[arg(long)]
    updates: Option<String>,
}
/// Finish a staged update before anything else starts — see `metal_update`'s
/// `swap` module. Never fatal: an install whose directory is not writable must
/// still start the app it already has, so an error is a line on stderr.
///
/// Skipped where there is no installable file to swap: the browser (no binary
/// on disk) and Android (the package manager owns the APK).
fn finish_pending_update_at_startup() {
    if cfg!(any(target_arch = "wasm32", target_os = "android")) {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("  update: cannot find the running binary: {e}");
            return;
        }
    };
    match metal_update::finish_pending_update(&exe) {
        Ok(true) => eprintln!("  update: staged binary swapped in ({})", exe.display()),
        Ok(false) => {}
        Err(e) => eprintln!("  update: cannot finish the pending update: {e}"),
    }
}

/// The desktop entry: `src/main.rs` calls this, and so does nothing else. The
/// app root is a library so the `cdylib` for Android (`android_main`, below)
/// links the same module tree the desktop binary does.
pub fn desktop_main() -> std::io::Result<()> {
    // The very first thing: finish an update the last run staged. On Windows
    // the running exe cannot be replaced, so an `apply` leaves `<exe>.new`
    // beside it and *this* call (the old process has already exited, so the
    // file is no longer locked) moves it into place. Everywhere else it is the
    // same recovery for a crash between the download and the first frame. It
    // must run before the window, the renderers or any file read: the binary
    // that starts the app is the one a person expects to be running.
    finish_pending_update_at_startup();

    let cli = Cli::parse();
    // `--updates <off|main|dev>` overrides the config file for this run. Set
    // once here, read live by `crate::update::channel` on every request.
    crate::update::set_channel_flag(cli.updates.clone().map(|v| v.trim().to_string()));
    // Before anything reads a file: `Outputs::load` runs inside `State::new`.
    if let Some(dir) = &cli.data {
        if let Err(e) = metal_vfs::create_dir_all(dir) {
            eprintln!("  cannot use {}: {e}", dir.display());
            std::process::exit(1);
        }
        output::set_data_root(dir);
    }

    // `S.U16`: monitor mode runs before any painter is built — a monitor
    // that tried to build renderers would be a renderer, and the slave
    // (O.8) renders nothing. Gated on `gpu` with the module itself.
    #[cfg(all(feature = "desktop", any(feature = "gpu", feature = "glow")))]
    if let Some(device) = cli.monitor.clone() {
        crate::monitor::run(cli.port, device, cli.display.clone());
        return Ok(());
    }
    // Without a window renderer there is no monitor to run: say so, rather
    // than serve as the master while the operator thinks this is a slave.
    #[cfg(not(all(feature = "desktop", any(feature = "gpu", feature = "glow"))))]
    if cli.monitor.is_some() {
        eprintln!("--monitor needs a window: build with --features gpu or glow");
        std::process::exit(1);
    }

    // B6: a one-way conversion, run once. `data/collections.json` stays
    // exactly where it is — this never deletes or rewrites it — because a
    // room with its mapping already written down is not a fixture to
    // discard. See `migrate_collections_to_graphs_once` for what "once"
    // means and why the original survives.
    migrate_collections_to_graphs_once();

    // S.U6/S.U15: with `gpu` and/or `glow` and without `--no-window`, the
    // window takes this thread and the listener moves to another. Every
    // other combination — neither feature, or `--no-window` — is the app
    // exactly as it was: serving from the main thread, no window, nothing
    // else running.
    #[cfg(any(feature = "gpu", feature = "glow"))]
    if !cli.no_window {
        return main_windowed(cli);
    }
    serve_forever(&cli, |_shared, _author| {})
}
/// Build the renderers and the state, bind, print the banner, and serve
/// until the process ends. Runs on the main thread when there is no
/// window, on its own thread when there is one; `on_ready` fires once the
/// listener is bound and the banner is out, handing back the very
/// `Arc<Mutex<Shared>>`/`Arc<Author>` this function is about to serve
/// from — the windowed `main` waits on it both to know the server is up
/// and to get the pair it hands `shell::window::run` for `call::door::Door`.
///
/// Built before binding, on purpose: a machine with no usable GPU adapter
/// must fail here, with a message, rather than serve a page of broken
/// images at a venue. On a build without the `gpu` feature the painter is
/// the CPU stand-in and cannot fail — see `paint.rs` for what that does and
/// does not stand in for.
fn serve_forever(
    cli: &Cli,
    on_ready: impl FnOnce(Arc<Mutex<Shared>>, Arc<Author>),
) -> std::io::Result<()> {
    eprintln!("MetalVJ — building the renderers…");
    let mut paint = paint::painter();
    let sim = open_input(cli);
    let mut state = State::new(paint.as_ref());
    if let Some(n) = &cli.node {
        state.local_node = n.clone();
        // O.8: the graph-owner identity is this same name.
        metal_graph::owner::set_local_identity(n);
        metal_perform::output::set_local_node(n);
    }
    state.port = cli.port;
    // P-D: the timecode readers and generators run with the show, not with a
    // poll of `GET /timecode`.
    timecode::start_supervisor(crate::output::data_root());
    // LAY.7: the device mode becomes a graph node before the first frame, so
    // every reader below — the plan compiler, the properties panel, the
    // routes — reads the graph and the file is only ever the pre-migration
    // fallback. Idempotent: a node already there is never overwritten.
    migrate_device_modes_to_graphs_once(&state);
    // LAY.8c: and the layer list becomes the graph's members in the same
    // breath — the mode pass just created the canvases, this fills them from
    // the pool's routing. Idempotent too: a boot with nothing new leaves every
    // device graph byte-identical.
    sync_device_graphs_once(&state, &paint.ids());
    // One graph per collection: every screen is an `output.device` node of the
    // one central collection, which publishes each screen's device graph — and
    // the per-screen collections an earlier install minted are folded into it.
    // Both are once, then idempotent.
    adopt_device_graphs_at(&crate::output::data_root());
    crate::merge_device_collections_at(&crate::output::data_root());
    // Conjunto C: the studio's renderers and state, built once up front for
    // the same reason `paint` is — a machine with no usable GPU adapter has
    // to say so here, not partway through serving `/author/`. G5: on the
    // painter's own device when it has one, so a solo picture is a texture.
    #[cfg(feature = "gpu")]
    let author = Arc::new(Author::with_gpu(metal_perform::paint::Paint::shared_gpu(
        paint.as_mut(),
    )));
    #[cfg(not(feature = "gpu"))]
    let author = Arc::new(Author::new());
    let shared = Arc::new(Mutex::new(Shared { state, paint, sim }));
    // The on-air model is warmed in the background from startup (see `warm`).
    warm::spawn(Arc::clone(&shared));
    warm::request(shared.lock().expect("shared lock poisoned").state.router.chosen);
    // The agent's executor (chat turns and `POST /agent/tool`) reaches the
    // live state through this door, so its door-bound tools work headless too.
    #[cfg(any(feature = "gpu", feature = "glow"))]
    call::install_agent_door(call::Door::new(Arc::clone(&shared), Arc::clone(&author)));
    // `POST /agent/tool` needs the install's bearer token; created on first start.
    #[cfg(feature = "agent")]
    {
        let token_path = metal_agent::auth::token_path();
        let token = metal_agent::auth::ensure_token(&token_path).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("agent token at {}: {e}", token_path.display()),
            )
        })?;
        call::set_agent_token(token);
    }

    let listener = TcpListener::bind((cli.host.as_str(), cli.port))?;
    let url = format!("http://{}:{}/", cli.host, cli.port);
    eprintln!("MetalVJ  {url}");
    {
        // Scoped: the lock is released before the accept loop below takes
        // it again per request, and before `on_ready` might hand `shared`
        // across a thread boundary.
        let guard = shared.lock().expect("shared lock poisoned");
        eprintln!(
            "  {} animations built, {} planned",
            guard.paint.model_count(),
            UNBUILT.len()
        );
        eprintln!("  painter: {}", guard.paint.describe());
        eprintln!(
            "  digest: {}{}",
            guard.sim.describe(),
            if guard.sim.is_real() {
                ""
            } else {
                "  ← stand-in, NOT the music in the room"
            }
        );
    }
    eprintln!("  data:   {}", output::data_root().display());

    eprintln!("  page:   off — the client is the native window; only `/out/` and the API are HTTP");
    on_ready(Arc::clone(&shared), Arc::clone(&author));

    // O.10b: the OSC reader, once the listener is up — it talks back to this
    // same process over loopback only. `--monitor` returned before here, so a
    // monitor never opens a second reader against the master's routes.
    input::spawn_osc_reader(
        Arc::clone(&shared),
        "127.0.0.1".to_string(),
        cli.port,
        cli.osc_port,
    );

    // O.10b: the MIDI reader, gated on `--features midi` — it links the system
    // ALSA sequencer, which not every host has headers for. Same loopback
    // contract as OSC: it only talks back to this process's routes.
    #[cfg(feature = "midi")]
    input::midi::spawn(
        Arc::clone(&shared),
        "127.0.0.1".into(),
        cli.port,
        cli.midi_port.clone(),
    );

    // P-C: screens with a sink (virtual camera, NDI…) are pushed from here.
    #[cfg(feature = "desktop")]
    call::videoio::spawn_pump(Arc::clone(&shared));

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if let Err(e) = serve(stream, &shared, &author) {
            // A browser closing a tab mid-frame is a broken pipe, and is not
            // worth a line. Anything else is.
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                eprintln!("  request failed: {e}");
            }
        }
    }
    Ok(())
}
/// Reads `--backend`/`FW_BACKEND` (the flag wins) and turns it into a
/// [`shell::window::Backend`] to force, or `Ok(None)` to leave the choice to
/// [`metal_render::gpu::probe`] (or, on a single-backend build, to whichever
/// one is compiled in). `S.U15`'s verification seam — `Err` is a startup
/// error, same weight as a bad `--port`, since a typo here would otherwise
/// silently fall through to the probe and hide the mistake.
#[cfg(any(feature = "gpu", feature = "glow"))]
fn resolve_forced_backend(cli: &Cli) -> Result<Option<shell::window::Backend>, String> {
    let raw = cli
        .backend
        .clone()
        .or_else(|| std::env::var("FW_BACKEND").ok());
    match raw {
        None => Ok(None),
        Some(s) => shell::window::Backend::parse(&s)
            .map(Some)
            .ok_or_else(|| format!("--backend/FW_BACKEND: not \"wgpu\" or \"glow\" (or not compiled in this build): {s:?}")),
    }
}
/// Reads `--theme`/`FW_THEME` (the flag wins) into the house style the shell
/// opens in, `Theme::phosphor` when neither is set — what opens today, so a
/// window opened the way it has always been opened still looks the same.
/// `Err` is a startup error with the same weight as a bad `--backend`: a
/// typo here would otherwise silently fall through to the dark theme.
#[cfg(any(feature = "gpu", feature = "glow"))]
fn resolve_theme(cli: &Cli) -> Result<shell::classes::Theme, String> {
    let raw = cli.theme.clone().or_else(|| std::env::var("FW_THEME").ok());
    match raw {
        None => Ok(shell::classes::Theme::phosphor()),
        Some(s) => shell::classes::Theme::from_name(&s).ok_or_else(|| {
            format!("--theme/FW_THEME: not \"dark\"/\"phosphor\" or \"light\"/\"daylight\": {s:?}")
        }),
    }
}
/// The windowed build: the listener on a background thread, the winit
/// event loop on this one (winit insists on the main thread on macOS and
/// iOS, so it is the server that moves). Behind `any(gpu, glow)`, `S.U15`
/// on: the window opens over whichever of `wgpu`/`egui_glow`
/// [`shell::window::resolve`] picks for this build.
///
/// Order: the server thread builds the renderers, binds and prints the
/// banner first; only when it says it is ready does the window open. If it
/// dies before that — bind failed, or the renderers panicked — this
/// returns the error and no window ever appears, which is the same "fail
/// loudly before serving" the single-threaded app had. If the window cannot
/// be opened (no display, no adapter for the surface), the app says so and
/// keeps serving headless. Closing the window ends the app: the process
/// exits and the listener thread with it.
#[cfg(any(feature = "gpu", feature = "glow"))]
fn main_windowed(cli: Cli) -> std::io::Result<()> {
    use std::sync::mpsc;

    let forced_backend = match resolve_forced_backend(&cli) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("  {e}");
            std::process::exit(1);
        }
    };

    // Before the thread spawn: `cli` moves into the server closure, so
    // anything else that reads it has to have read it by here.
    let theme = match resolve_theme(&cli) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  {e}");
            std::process::exit(1);
        }
    };

    // A panic on the server thread used to be a panic in `main` — the
    // process died with the message. Keep that: without this hook the
    // thread would die alone and the window would sit over a dead server.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        std::process::exit(101);
    }));

    // `S.U7b`: `on_ready` now carries the `Arc<Mutex<Shared>>`/`Arc<Author>`
    // pair the server thread just built, not just a bare "I'm up" signal —
    // this is the one new thing that had to cross from that thread to this
    // one for `call::door::Door` to exist at all.
    let (ready_tx, ready_rx) = mpsc::channel::<(Arc<Mutex<Shared>>, Arc<Author>)>();
    let server = std::thread::Builder::new()
        .name("metal-dash http".into())
        .spawn(move || {
            serve_forever(&cli, |shared, author| {
                let _ = ready_tx.send((shared, author));
            })
        })?;

    let (shared, author) = match ready_rx.recv() {
        Ok(pair) => pair,
        Err(_) => {
            // The sender was dropped without sending: `serve_forever`
            // returned before `on_ready` — that is a bind error, and it is
            // ours to report.
            return match server.join() {
                Ok(result) => result,
                Err(_) => Err(std::io::Error::other(
                    "the server thread died before it was ready",
                )),
            };
        }
    };

    let door = call::Door::new(shared, author);
    let backend = shell::window::resolve(forced_backend);
    eprintln!("  paint backend: {backend:?}");
    match shell::window::run(theme, door, backend) {
        shell::window::Outcome::Closed => {
            eprintln!("  window closed — stopping");
            Ok(())
        }
        shell::window::Outcome::NoWindow(why) => {
            eprintln!("  no window: {why}");
            eprintln!("  serving headless — the dashboard is still at the address above");
            match server.join() {
                Ok(result) => result,
                Err(_) => Err(std::io::Error::other("the server thread died")),
            }
        }
    }
}
/// What a `null` `audio.live` source means (plan Design 3): `--listen
/// <source>` seeds it, bare `--listen` or nothing is the system default.
/// Only the seed; every node naming a source opens its own (`Listeners`).
///
/// **A failure here is loud and does not stop the app**, and is not hidden
/// behind the stand-in either: the clock reads silence, the nodes on that
/// source read a zero row and turn red, and the banner says so.
fn open_input(cli: &Cli) -> Listeners {
    let seed = match cli.listen.as_deref() {
        None | Some("") => listeners::SourceKey::Default,
        Some(pick) => listeners::SourceKey::from_pick(pick),
    };
    let pool = Listeners::new(seed.clone());
    if !pool.is_open(&seed) {
        let what = if seed.as_pick().is_empty() {
            "the default input"
        } else {
            seed.as_pick()
        };
        eprintln!(
            "  cannot listen to {what} — audio nodes on it read silence until a source opens"
        );
    }
    pool
}

/// C4: `data/graphs/*.json` as a flat list, for the merged page's
/// collections panel. Read-only and minimal on purpose — one graph's full
/// state (nodes, edges, layout, lint) is `GET /graph?id=`, below; this route
/// only exists so a list of collections can be painted without loading every
/// graph in full. A file that fails to parse is skipped rather than failing
/// the whole listing: one corrupt graph must not blank the list for every
/// other one.
fn list_graphs() -> Vec<Value> {
    let dir = metal_perform::engine::graphs_dir(&output::data_root());
    let Ok(entries) = metal_vfs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut graphs: Vec<Value> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| metal_vfs::read_to_string(e.path()).ok())
        .filter_map(|text| metal_graph::Graph::from_json(&text).ok())
        .map(|g| json!({ "id": g.id, "name": g.name, "domain": g.domain }))
        .collect();
    graphs.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    graphs
}

/// §16.4: the node catalog, served exactly as [`metal_graph::NodeKind::catalog`]
/// returns it — 34 generators (32 of §2 plus `scene3d`, H.1, and `dla`) and
/// 16 effects derived from `data/schema.json`, plus the twelve fixed kinds.
///
/// **Why this is not `/graph/node`, and must not grow into it.** This reads a
/// pure function of the manifest. Nothing here creates, mutates or persists a
/// node; there is no graph on the other end of it. The node/edge mutation
/// surface is `POST /graph/node`/`POST /graph/edge`, below — kept separate so
/// this route can stay exactly what it says: "what kinds exist", answerable
/// with no graph loaded at all.
///
/// **Why the page was not just handed a literal list instead.** The same
/// reason `catalog()` itself reads the manifest rather than hard-coding one:
/// this repo already shipped the bug where a hand-written model list and
/// `data/schema.json` silently disagreed about three models (see
/// `manifest.rs`, and `metal-graph`'s own `catalog_matches_schema` test). A
/// second copy in the page would rot the same way, and the symptom — a menu
/// entry that creates a source pointed at a model index that no longer means
/// what the menu thought — is the silent, in-range, wrong failure §3.3 and
/// §11.7 both name.
///
/// **Why the manifest is read per request and not cached.** This is a
/// top-bar menu a human opens, not a pump: the page asks once on load. A
/// `OnceLock` here would buy nothing measurable and would need a story for
/// what happens when `data/schema.json` changes under a running app, which
/// reading it fresh does not.
///
/// An unreadable or unparseable `schema.json` is reported as such rather than
/// answered with an empty list. Empty would render as a catalog with nothing
/// in it — which is a lie of the exact kind §16.4 is about, and the caller
/// cannot tell it apart from "this build has no models".
fn node_catalog() -> Result<Vec<metal_graph::NodeKind>, String> {
    let path = output::data_root().join("schema.json");
    let text = metal_vfs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let manifest = metal_schema::Manifest::parse(&text)
        .map_err(|e| format!("{} did not parse: {e}", path.display()))?;
    Ok(metal_graph::NodeKind::catalog(&manifest))
}

#[cfg(feature = "agent")]
/// H.8: the provider-config overlay's backend half, `GET`/`POST /ai-config`.
///
/// This is the *only* route that touches `data/ai.json`, on purpose — PARTE
/// IV's self-consistency rule says the agent loop's own future read of this
/// config (H.3/H.4) has to go through the same door the UI overlay writes
/// through, not a second one. `metal_agent::config::ProviderConfig` already
/// carries the non-negotiable rule (H.3: never log the raw key); this layer's
/// only added job is making sure the raw key never round-trips back to the
/// browser either — `GET` always answers with the masked tail, never `key`.
///
/// S.AH: `preset`/`url`/`model` are governed by `[agent.provider]` in
/// `~/.config/metalvj/config.toml` once that table exists — `load`/`save`
/// read and mirror it themselves (see `metal-agent/src/config.rs`). The `_at`
/// helpers take the `config.toml` path explicitly only so the tests below
/// can point them at a scratch file instead of the developer's real one.
fn ai_config_path() -> std::path::PathBuf {
    output::data_root().join("ai.json")
}

#[cfg(feature = "agent")]
/// `{ "configured": bool, "url", "model", "preset", "key_masked" }`. All four
/// fields are omitted (not just blanked) when nothing has been saved yet, so
/// the page can tell "no provider configured" apart from "configured with an
/// empty model name" without a magic-string convention.
fn ai_config_get_reply_at(
    path: impl AsRef<std::path::Path>,
    config_toml: &std::path::Path,
) -> Result<Value, String> {
    match metal_agent::config::ProviderConfig::load_at(path, config_toml)
        .map_err(|e| format!("cannot read ai.json/config.toml: {e}"))?
    {
        None => Ok(json!({ "configured": false })),
        Some(cfg) => Ok(json!({
            "configured": true,
            "url": cfg.url,
            "model": cfg.model,
            "preset": cfg.preset,
            "key_masked": cfg.masked_key(),
        })),
    }
}

#[cfg(feature = "agent")]
/// Applies a `POST /ai-config` body to disk and returns the same masked shape
/// `ai_config_get_reply` does, so the page can repaint from the response
/// without a follow-up `GET`.
///
/// `key` is optional in the body on purpose: the page shows a masked
/// placeholder rather than the real value (see the route's own doc comment),
/// so "the operator didn't touch the key field" and "the operator wants to
/// clear the key" have to be distinguishable. An absent or empty `key` keeps
/// whatever was already on disk; the only way to change it is to type a new,
/// non-empty one.
fn ai_config_post_at(
    path: impl AsRef<std::path::Path>,
    config_toml: &std::path::Path,
    body: &Value,
) -> Result<Value, String> {
    let preset: metal_agent::config::Preset = serde_json::from_value(body["preset"].clone())
        .map_err(|_| "invalid or missing preset".to_string())?;
    let url = body["url"].as_str().unwrap_or_default().to_string();
    let model = body["model"].as_str().unwrap_or_default().to_string();
    let path = path.as_ref();
    let existing = metal_agent::config::ProviderConfig::load_at(path, config_toml)
        .map_err(|e| format!("cannot read ai.json/config.toml: {e}"))?;
    let key = match body["key"].as_str() {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => match &existing {
            // A provider that exists only in `config.toml` (S.AH) loads with
            // an empty key — there is nothing on disk to keep, so it is the
            // same 400 as no file at all.
            Some(cfg) if !cfg.key.is_empty() => cfg.key.clone(),
            _ => return Err("expected a key".to_string()),
        },
    };
    // H.6: `stt`/`tts` are carried over from the existing file instead of being
    // left as `None`. This route only edits the *text* provider — the AI config
    // overlay (H.8) shows no voice — so rebuilding the struct from zero here
    // would silently erase a voice configuration the user wrote by hand in
    // `ai.json`. Saving replaces the whole file, not a merge, and that is why
    // the merge has to happen here.
    let (stt, tts) = match &existing {
        Some(cfg) => (cfg.stt.clone(), cfg.tts.clone()),
        None => (None, None),
    };
    let cfg = metal_agent::config::ProviderConfig {
        url,
        model,
        key,
        preset,
        stt,
        tts,
    };
    cfg.save_at(path, config_toml)
        .map_err(|e| format!("cannot write ai.json/config.toml: {e}"))?;
    Ok(json!({
        "configured": true,
        "url": cfg.url,
        "model": cfg.model,
        "preset": cfg.preset,
        "key_masked": cfg.masked_key(),
    }))
}

#[cfg(feature = "agent")]
/// `S.U14`: which of `metal_agent::harness::TOOL_CATALOG`'s tools the sidebar's
/// per-tool toggle has turned off, keyed by name. Its own file for the same
/// reason `ai_config_path` has one — rule 6's "no second state of the
/// world" rule means the sidebar's toggle and the agent's own future read of
/// it (`S.X`) have to agree on one place on disk, not a shell-local bool a
/// resize would forget.
fn ai_tools_path() -> std::path::PathBuf {
    output::data_root().join("ai_tools.json")
}

#[cfg(feature = "agent")]
/// Absent file, or an absent name in it, both mean "the default policy" — a
/// fresh install starts with the whole implemented catalog available, the
/// destructive tools defaulting to `ask` (the flags in
/// [`metal_agent::tool_schemas`]). Legacy `{name: bool}` maps load unchanged
/// (`policy::resolve` folds `true → auto`, `false → off`).
fn ai_tools_load(path: impl AsRef<std::path::Path>) -> Value {
    metal_vfs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}))
}

#[cfg(feature = "agent")]
fn ai_tools_save(path: impl AsRef<std::path::Path>, map: &Value) -> Result<(), String> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(path, serde_json::to_string_pretty(map).unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[cfg(feature = "agent")]
/// H.4's own route correspondence (`crates/metal-agent/src/harness.rs`'s module
/// doc: *"a thin wrapper over a route that already exists"*), moved to the
/// one place that can name both `metal_agent::harness::TOOL_CATALOG` and
/// `crate::call::ROUTES`/`crate::call::author::AUTHOR_ROUTES` without a
/// `shell/` file ever importing `metal_agent` directly (gate 1). A path here
/// that starts with `/author` is exactly what a panel would call through
/// the door (`crate::call::door::Door::call`'s own `/author` prefix
/// bifurcation) — never the bare `AUTHOR_ROUTES` entry's path on its own.
///
/// Groups of mappings, each flagged rather than guessed at:
/// - `sleep`, `locate`, `capture_camera_frame`, `bake_clip`, `save_shader`,
///   `take_graph`, `release_graph`, `graph_owner`, `capture_output`:
///   implemented inside the agent process, or by `metal-dash`'s own executor,
///   with no route of their own. They map to `POST /agent/tool`, the generic
///   route that runs any catalog tool through the chat's executor.
/// - `get_params`/`set_params`, and `rasterize_overlay`: the closest
///   *registered* door surface, not a byte-identical one.  `get_params`/
///   `set_params` read/patch one node's free-form fields —`POST /graph/node`
///   (`op: "flag"`/`"layer"`) is the nearest existing op, not a generic
///   patch. `rasterize_overlay`'s own `metal-dash` executor
///   (`author::models::agent_tools::AgentHarness::rasterize_overlay`) is
///   itself a documented placeholder ("in a real implementation, this would
///   POST to /overlay/rasterize" — that route does not exist), so this maps
///   it to `POST /overlay`, the real route in the same family, rather than
///   inventing the placeholder's imagined one.
fn tool_route(name: &str) -> Option<(&'static str, &'static str)> {
    // Routes whose body does not take the tool's own arguments: checked by
    // calling each with the schema's args (`which graph`, `no such op`,
    // `expected {timeline}`, missing `model`/`name`). They run through the
    // executor chat uses (`POST /agent/tool`), which adapts the arguments.
    const EXECUTOR_ROUTED: &[&str] = &[
        "get_env",
        "list_nodes",
        "get_params",
        "read_shader",
        "get_io",
        "create_node",
        "delete_node",
        "connect_nodes",
        "set_params",
        "raster",
        "embed_ai",
        "bind_param",
        "set_param",
        "edit_mask",
        "set_switch",
        "solid_color",
        "rasterize_overlay",
        "edit_layer",
        "import_segment",
        "move_segment",
        "remove_segment",
        "set_layer_mix",
        "set_keyframe",
        "attach_digest",
        "set_palette",
        "edit_scene",
        "add_switch",
        "remove_switch",
        "attach_source",
        "insert_node",
        // M4 of the materials lote: the schema names `graph`/`node`/`part` +
        // the material knobs; `POST /graph/node` wants `op` + `config`.
        "set_material",
        // The door-bound authoring tools: their schemas name `outputs`,
        // `venue`/`surface`, which only the executor's adapters turn into
        // the route's own body (`device_ids`, `op: "surface"`).
        "create_mask_grid",
        "test_mask_grid",
        "edit_venue_surface",
        // The venue tools' schemas name `venue`, `fixture`, `target`, none
        // of which `POST /venue` takes: that route wants `op` and `id`. `look_venue`
        // is also not `GET /venue` (that route lists venues). The executor
        // owns each of them.
        "project_venue",
        "calibrate_projector",
        "blend_edges",
        "aim_fixture",
        "look_venue",
        "scan_venue",
        "discover_fixture",
        "solve_head",
        // D2: the GDTF importer's schema names `path`/`bytes`; `POST /fixtures`
        // wants `op` + `path`/`bytes`, which the executor adapts.
        "import_gdtf",
        // P-C: the schema names `device_id`; the route wants `op`/`id`.
        "set_output_sink",
    ];
    if EXECUTOR_ROUTED.contains(&name) {
        return Some(("POST", call::AGENT_TOOL_ROUTE));
    }
    match name {
        "list_tools" => Some(("GET", "/ai-config")),
        "search_nodes" => Some(("GET", "/catalog")),
        // A1/A3.3: the embedded reaction is a setting in the source, over the
        // same `POST /graph/node` op its properties toggle posts.
        // S.AM: the train gesture of an `ai.*` node — the same route its
        // `entrenar` button posts (`POST /ai/train { node }`).
        "train_ai" => Some(("POST", "/ai/train")),
        "list_switches" => Some(("GET", "/switch")),
        "list_transitions" => Some(("GET", "/transitions")),
        // V2b: the tick-level switch is a shell setting; both tools use the
        // one `/settings` route the settings overlay uses (rule 6).
        // The sync read tools: `metal-sync`'s status, peers and log, forwarded by
        // the door (`call::sync`), the routes the Dispositivos page reads.
        "sync_status" => Some(("GET", "/sync/status")),
        "sync_peers" => Some(("GET", "/sync/peers")),
        "sync_log" => Some(("GET", "/sync/log")),
        // P-D: Art-Net discovery and the timecode reading.
        "artnet_nodes" => Some(("GET", "/artnet/nodes")),
        "artnet_in_status" => Some(("GET", "/artnet/in")),
        "timecode_status" => Some(("GET", "/timecode")),
        "get_tick_level" => Some(("GET", "/settings")),
        "set_tick_level" => Some(("POST", "/settings")),
        // H5: the head-tracking gesture — the same `POST /sensor {op:"set"}`
        // the `sensor.yolo` panel's controls post.
        "set_head_tracking" => Some(("POST", "/sensor")),
        // P-G: the preview bar's "Export frame".
        "export_frame" => Some(("POST", "/frame/export")),
        // E1: the offline video export job.
        "export_video" => Some(("POST", "/export/video")),
        "list_video_io" => Some(("GET", "/videoio")),
        // `S.O.1`: real now — its own route, not a placeholder's imagined one.
        "blackout" => Some(("POST", "/overlay/blackout")),
        // `S.O.2`: real now — `POST /overlay {op:"solid"}`.
        // S.D (O.3): a segment's `SegmentMix` — the tool resubmits the whole
        // segment through the timeline's own segment route, never `/layers`
        // (`author::models::agent_tools::AgentHarness::set_layer_mix`).
        // O.1's authoring half: the one route that writes a mask.
        // `capture_output` is not here: it reads `GET /render/<device>/<node>/
        // <frame>` through the executor below, which builds the path from its
        // arguments (a bare `GET /render/` body cannot name a node).
        "listen_status" => Some(("GET", "/listen/status")),
        // Param edges (`metal_graph::bind`): the properties panel's I/O read and
        // its two writes, `op: "bind"` and `op: "param"`.
        // K: the palette node's write — the same `/author/palettes {op:"set"}`
        // its folder posts (the exported route is `/author/palettes`).
        // Video scenes: the gallery's read, the book's writes.
        "list_scenes" => Some(("GET", "/author/scenes")),
        // The sequence editor's dataset picker: the list, and one part with `part`.
        "list_dataset" => Some(("GET", "/author/dataset")),
        // D1: the venue's fixtures — the same `POST /venue {op:"fixture"}`
        // the Settings "Fixtures" page posts (rule 6). The MCP adapter forwards
        // arguments verbatim (no `op`), so the agent executor adds it.
        "set_fixture" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        // D3: the universe bus, `POST /outputs {op:"dmx-bus"}`; the executor
        // adds the `op` for the same reason.
        "dmx_bus" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        // Gap closure: the routes the human panels already read and write.
        "perf_report" => Some(("GET", "/perf")),
        "get_grid_layouts" => Some(("GET", "/settings")),
        "set_grid_layouts" => Some(("POST", "/settings")),
        "export_status" => Some(("GET", "/export")),
        "export_control" => Some(("POST", "/export")),
        "list_palettes" => Some(("GET", "/author/palettes")),
        // Executor-routed: the chat's executor posts each tool's own route (rule 6).
        "list_kinds" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "add_effect" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "remove_effect" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "edit_output" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "resolve_draft" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "edit_venue_object" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "add_screen" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "add_projection" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "set_surface_form" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "set_venue_sectors" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "move_venue_item" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "projection_area" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_scene" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "add_stage" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "add_truss" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "add_dummy" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_spline" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "place_mask_in_space" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_boolean" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_hit" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_report" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_view" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_assets_search" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "venue_asset_import" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "play_sequence" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "edit_loops" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "transport" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "overlay_control" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "colour_apply" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        "focus" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        // F9: the per-fixture node's arguments (`fixture`/`colour_from`/
        // `model`) are not the route body, so it runs through the chat's
        // executor like the other in-process tools; it posts `POST /graph/node`
        // and `POST /graph/edge` itself (rule 6).
        "add_fixture_node" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        // §9.4 V3: the script node's own source over the `/script` route the
        // properties editor walks (rule 6).
        "read_script" => Some(("GET", "/script/")),
        "save_script" => Some(("POST", "/script")),
        // In-process tools: no route of their own; `POST /agent/tool` runs them
        // through the chat's executor (`crate::chat::agent_tool_at`), policy
        // applied. metal-mcp wraps the arguments as `{name, args, allow_ask}`.
        "sleep"
        | "locate"
        | "capture_camera_frame"
        | "bake_clip"
        | "save_shader"
        | "take_graph"
        | "release_graph"
        | "graph_owner"
        | "capture_output" => Some(("POST", call::AGENT_TOOL_ROUTE)),
        _ => None,
    }
}

#[cfg(feature = "agent")]
/// `GET /ai-config`'s `tools` array, and the tools-only half of
/// `POST /ai-config` reads this same shape back after saving — one tool per
/// `TOOL_CATALOG` entry, `enabled` folding in both "does this build actually
/// implement it" and "did the operator turn it off", and `route` naming the
/// exact `(method, path)` [`tool_route`] maps it to (`null` for the
/// documented gap list on that function's own doc comment).
fn ai_tools_reply(tools_path: impl AsRef<std::path::Path>) -> Value {
    let saved = ai_tools_load(tools_path);
    let tools: Vec<Value> = metal_agent::harness::TOOL_CATALOG
        .iter()
        .map(|(name, desc, implemented)| {
            // `S.X` D10: the per-tool setting is now a policy (`auto`/`ask`/
            // `off`), with the default from the schema flags. `enabled` stays
            // as the summed "this tool will be offered to the model" flag the
            // old readers (and tests) expect.
            let policy = if *implemented {
                metal_agent::harness::effective_policy(name, &saved)
            } else {
                metal_agent::chat::policy::ToolPolicy::Off
            };
            let enabled = *implemented && policy != metal_agent::chat::policy::ToolPolicy::Off;
            let route = tool_route(name);
            json!({
                "name": name,
                "description": desc,
                "implemented": implemented,
                "enabled": enabled,
                "policy": policy.wire(),
                "route": route.map(|(m, p)| json!({ "method": m, "path": p })).unwrap_or(Value::Null),
            })
        })
        .collect();
    // The three capability permissions, answered even when unset: `ask` is
    // what the harness enforces for them, so the sidebar shows the truth.
    let permissions: serde_json::Map<String, Value> = metal_agent::harness::Capability::ALL
        .iter()
        .map(|cap| {
            let wire = metal_agent::harness::permission_wire(cap.policy(&saved));
            (cap.key().to_string(), json!(wire))
        })
        .collect();
    let instructions = saved["instructions"].as_str().unwrap_or("");
    json!({
        "tools": tools,
        "permissions": permissions,
        "instructions": instructions,
    })
}

#[cfg(feature = "agent")]
/// Applies the sidebar's AI settings to `ai_tools.json`:
/// - `{ "tools": { name: "auto"|"ask"|"off" } }` (or the legacy `{name: bool}`),
///   which silently refuses a tool `TOOL_CATALOG` marks `implemented: false`;
/// - `{ "permissions": { "internet"|"dispositivos"|"archivos": "allow"|"ask"|"deny" } }`;
/// - `{ "instructions": "..." }`, the operator's free text for the system prompt.
///
/// Each key touches only its own part of the file, so a permission pick never
/// rewrites the tool table and vice versa.
fn ai_tools_post_at(path: impl AsRef<std::path::Path>, body: &Value) -> Result<Value, String> {
    let path = path.as_ref();
    let mut saved = ai_tools_load(path);
    if !saved.is_object() {
        saved = json!({});
    }
    if let Some(patch) = body["permissions"].as_object() {
        for cap in metal_agent::harness::Capability::ALL {
            let parsed = patch
                .get(cap.key())
                .and_then(Value::as_str)
                .and_then(metal_agent::harness::permission_policy);
            if let Some(policy) = parsed {
                saved["permissions"][cap.key()] =
                    json!(metal_agent::harness::permission_wire(policy));
            }
        }
    }
    if let Some(text) = body["instructions"].as_str() {
        saved["instructions"] = json!(text);
    }
    if let Some(patch) = body["tools"].as_object() {
        for (name, v) in patch {
            let implemented = metal_agent::harness::TOOL_CATALOG
                .iter()
                .any(|(n, _, imp)| *n == name && *imp);
            if !implemented {
                continue;
            }
            let parsed = v
                .as_str()
                .and_then(metal_agent::chat::policy::ToolPolicy::parse)
                .or_else(|| {
                    v.as_bool().map(|b| {
                        if b {
                            metal_agent::chat::policy::ToolPolicy::Auto
                        } else {
                            metal_agent::chat::policy::ToolPolicy::Off
                        }
                    })
                });
            if let Some(policy) = parsed {
                saved["policy"][name] = json!(policy.wire());
            }
        }
    }
    ai_tools_save(path, &saved)?;
    Ok(ai_tools_reply(path))
}

#[cfg(feature = "agent")]
/// `S.U14`: the sidebar's prompt transcript — `data/ai_log.json`, a plain
/// JSON array of `{ "role", "content", "at" }`, append-only from
/// `POST /ai/ask`, read whole by `GET /ai/log`. **This route does not call a
/// model.** `plan-opus.md` names the harness's own turn loop
/// (`metal_agent::agent_loop::run_turn`, the per-preset adapter it still
/// needs) as `S.X`'s job, not this one's — `S.U14`'s scope is the sidebar
/// and rule 6's parity, not standing up the first real network call to a
/// provider. Recording what was asked, honestly, rather than faking an
/// answer, is the same "flagged, not faked" trade `harness.rs`'s own module
/// doc makes for the tools it does not implement.
fn ai_log_path() -> std::path::PathBuf {
    output::data_root().join("ai_log.json")
}

#[cfg(feature = "agent")]
fn ai_log_load(path: impl AsRef<std::path::Path>) -> Vec<Value> {
    metal_vfs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[cfg(feature = "agent")]
fn ai_ask_post_at(path: impl AsRef<std::path::Path>, body: &Value) -> Result<Value, String> {
    let prompt = body["prompt"].as_str().unwrap_or_default();
    if prompt.trim().is_empty() {
        return Err("expected a non-empty `prompt`".to_string());
    }
    let path = path.as_ref();
    let mut log = ai_log_load(path);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    log.push(json!({ "role": "user", "content": prompt, "at": at }));
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(path, serde_json::to_string_pretty(&log).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(json!({ "queued": true, "log": log }))
}

/// output is ever turned into JSON, the same discipline `Config` documents
/// for `serde_json` staying out of that crate's public surface.
///
/// Every primitive carries its **tag** (N: the Wire canvas). A painter reads
/// the tag to decide how to draw — a node box, a header band, a port dot, a
/// param dot, a value field, a swatch, a model tile, the `▸ n more` row, a row
/// label, a title, a value readout — and never re-derives which rectangle is
/// which from its size (`Layout::primitives`' own doc). A cable carries its
/// endpoints and its type (`image`/`control`/`param`), so a painter styles a
/// param edge differently from an image cable without re-reading the graph.
fn primitive_json(p: &metal_graph::layout::Primitive) -> Value {
    use metal_graph::layout::{Primitive, RectTag, TaggedRect, TextTag};
    match p {
        Primitive::Rect(TaggedRect { rect, tag }) => match tag {
            RectTag::Node { node } => json!({
                "type": "rect", "tag": "node", "node": node,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Header => json!({
                "type": "rect", "tag": "header",
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Port { node, port, out } => json!({
                "type": "rect", "tag": "port",
                "node": node, "port": port, "out": out,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
                "cx": rect.x + rect.w / 2.0, "cy": rect.y + rect.h / 2.0,
            }),
            RectTag::ParamDot { node, name } => json!({
                "type": "rect", "tag": "param_dot",
                "node": node, "name": name,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
                "cx": rect.x + rect.w / 2.0, "cy": rect.y + rect.h / 2.0,
            }),
            RectTag::ValueField { node, name } => json!({
                "type": "rect", "tag": "value_field",
                "node": node, "name": name,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Swatch { node, name, rgb } => json!({
                "type": "rect", "tag": "swatch",
                "node": node, "name": name,
                "color": [rgb[0], rgb[1], rgb[2]],
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Tile { node } => json!({
                "type": "rect", "tag": "tile",
                "node": node,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::More { node } => json!({
                "type": "rect", "tag": "more",
                "node": node,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Chip { node, source, ai } => json!({
                "type": "rect", "tag": "chip",
                "node": node, "source": source, "ai": ai,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
        },
        Primitive::Path(p) => json!({
            "type": "path",
            "edge": p.edge,
            "from": { "node": p.from.0, "port": p.from.1 },
            "to": { "node": p.to.0, "port": p.to.1 },
            "kind": match p.kind {
                metal_graph::layout::EdgeKind::Image => "image",
                metal_graph::layout::EdgeKind::Control => "control",
                metal_graph::layout::EdgeKind::Param => "param",
            },
            "p0": [p.path.p0.x, p.path.p0.y],
            "p1": [p.path.p1.x, p.path.p1.y],
            "p2": [p.path.p2.x, p.path.p2.y],
            "p3": [p.path.p3.x, p.path.p3.y],
        }),
        Primitive::Text(t) => json!({
            "type": "text",
            "x": t.x, "y": t.y, "content": t.content,
            "tag": match t.tag {
                TextTag::Title => "title",
                TextTag::Row => "row",
                TextTag::Value => "value",
                TextTag::More => "more",
                TextTag::Plain => "plain",
            },
        }),
    }
}

fn issue_json(i: &metal_graph::Issue) -> Value {
    json!({
        "severity": match i.severity { metal_graph::Severity::Error => "error", metal_graph::Severity::Warning => "warning" },
        "node": i.node,
        "message": i.message,
    })
}

/// Thin wrapper over [`metal_graph::Graph::add_edge_checked`] (H.4: moved into
/// `metal-graph` itself so the AI harness's `connect_nodes`/`create_node` tools
/// share the exact same validated-connect logic this route uses, instead of
/// a second copy of "add, lint, maybe roll back").
fn add_edge_checked(
    g: &mut metal_graph::Graph,
    from: metal_graph::Endpoint,
    to: metal_graph::Endpoint,
    catalog: &[metal_graph::NodeKind],
) -> Result<String, Vec<String>> {
    g.add_edge_checked(from, to, catalog)
}

/// The reply every `/graph*` mutation converges on: the graph as saved, its
/// primitives (for the client to repaint from, never computing layout
/// itself), and every lint issue — errors and warnings together, per
/// `lint`'s own doc comment on why it is never a `bool`.
/// The `GET /graph/hit` answer for a press at `(x, y)`: the same function the
/// route runs, reachable from the shell's tests without a `call::` path.
#[cfg(test)]
fn graph_hit_reply(g: &metal_graph::Graph, catalog: &[metal_graph::NodeKind], x: f32, y: f32) -> Value {
    call::graph::hit_reply(&g.layout(catalog), x, y)
}

fn graph_reply(g: &metal_graph::Graph, catalog: &[metal_graph::NodeKind]) -> Value {
    let layout = g.layout(catalog);
    let issues = g.lint(catalog);
    json!({
        "graph": g,
        "primitives": layout.primitives().iter().map(primitive_json).collect::<Vec<_>>(),
        "issues": issues.iter().map(issue_json).collect::<Vec<_>>(),
    })
}

/// B4: one generator's sixteen effects, exactly as the config panel shows
/// them — [`metal_graph::effects::view`]'s present effects first, in chain
/// order (§3.2: the order *is* the graph order), then every other `fx.*`
/// kind the catalog knows about, marked `off`. Built fresh from the graph on
/// every call — never cached, never stored beside it — which is the whole
/// answer to §3.2's warning that a second copy of this list is how "which
/// effects are on" and "what the graph says" end up disagreeing.
fn effects_reply(
    graph: &metal_graph::Graph,
    gen_node: &str,
    catalog: &[metal_graph::NodeKind],
) -> Value {
    let present = metal_graph::effects::view(graph, gen_node);
    let mut seen: std::collections::HashSet<String> =
        present.iter().map(|v| v.effect.clone()).collect();
    let mut out: Vec<Value> = present.iter().map(effect_view_json).collect();
    for k in catalog {
        if k.family != metal_graph::NodeFamily::Effect {
            continue;
        }
        // S.W: `fx.text.*` is a *glyph* vocabulary, not a pixel chain step, so
        // it is not one of the effects a generator's own panel offers.
        if k.kind.starts_with("fx.text.") {
            continue;
        }
        // `fx.forge` is not an amount to switch on: it is a shader, named on
        // the node when it is created (`create_node` with `after`). A panel
        // row that switched it "on" would make a node with nothing to run.
        // One already in the chain is in `present` above and shows there.
        if k.kind == metal_graph::FORGE_EFFECT_KIND {
            continue;
        }
        // K: `fx.speed`/`fx.move` are not `Post` amounts either — they act on
        // the clock and on the picture, not on a generator's row, so a panel
        // toggle for them would be a control that does nothing. Same rule as
        // `fx.forge`: offered only when already in the chain (`present`).
        if k.kind == format!("fx.{}", metal_graph::SPEED_EFFECT_ID)
            || k.kind == format!("fx.{}", metal_graph::MOVE_EFFECT_ID)
        {
            continue;
        }
        let name = k.kind.strip_prefix("fx.").unwrap_or(&k.kind).to_string();
        if seen.insert(name.clone()) {
            out.push(json!({ "effect": name, "node": Value::Null, "state": "off" }));
        }
    }
    json!({ "node": gen_node, "effects": out })
}

fn effect_view_json(v: &metal_graph::effects::EffectView) -> Value {
    let mut j = json!({ "effect": v.effect, "node": v.node });
    match v.state {
        metal_schema::EffectState::Off => {
            j["state"] = json!("off");
        }
        metal_schema::EffectState::Reactive => {
            j["state"] = json!("reactive");
            // `control_source` is `None` only when disconnected, which
            // `state_of`'s own rule already rules out for `Reactive` — the
            // `None` arm here is defensive, not reachable, per E4.
            j["source"] = json!(match v.source {
                Some(metal_graph::effects::ControlSource::Row) => "row",
                Some(metal_graph::effects::ControlSource::RawIngest) | None => "raw",
            });
        }
        metal_schema::EffectState::Fixed(val) => {
            j["state"] = json!("fixed");
            j["value"] = json!(val);
        }
    }
    j
}

/// `shared` is `&Mutex<Shared>` rather than `&Arc<Mutex<Shared>>` — deref
/// coercion from the accept loop's `&Arc<Mutex<Shared>>` — because this
/// function only ever borrows it for the length of one request; it never
/// needs to clone the `Arc` itself the way `call::door::Door` does to carry
/// one past this function's return. Locked once, right before the tail that
/// needs it, so the assets/`/author` paths above never wait on a lock they
/// do not use.
fn serve(mut stream: TcpStream, shared: &Mutex<Shared>, author: &Author) -> std::io::Result<()> {
    let Some(req) = Request::read(&stream)? else {
        return Ok(());
    };

    if let Some((kind, bytes)) = assets::get(&req.path) {
        return metal_web::cached(&mut stream, kind, bytes);
    }

    // C2: `/author/` used to be a second page; the merge absorbed it into
    // `PAGE` itself (the `authorVeil`), so both spellings just send whoever
    // asks there. Built from the un-stripped path, and relative rather than
    // `Location: /`, because a root-absolute `Location` is resolved by the
    // browser against the *origin*, not wherever a reverse proxy mounted
    // this app (the reason the old page carried `ROOT`/`api` helpers). `.` and `../` are not interchangeable here: the
    // trailing slash changes which segment a relative reference is resolved
    // against.
    if req.path == "/author" {
        return metal_web::redirect(&mut stream, ".");
    }
    if req.path == "/author/" {
        return metal_web::redirect(&mut stream, "../");
    }

    // C1: everything the absorbed studio answers, under one prefix — see
    // `author::mod`. Checked before `call::ROUTES` below so the studio's own
    // `/state`, `/select`, `/preset` (colliding names, kept because renaming
    // them would break `fw-studio`'s own page mid-merge) never reach this
    // dispatcher's own rows for those same paths. `the cleanup`: the studio's own
    // routes are a second table now (`call::author::AUTHOR_ROUTES`), walked
    // by `Author::dispatch`, so this is the same shape as the `call::ROUTES`
    // tail below rather than a raw-socket handoff.
    if req.path == "/author" || req.path.starts_with("/author/") {
        let rest = req.path.strip_prefix("/author").unwrap_or("");
        let inner_path = if rest.is_empty() {
            "/".to_string()
        } else {
            rest.to_string()
        };
        let inner_req = Request {
            method: req.method.clone(),
            path: inner_path,
            query: req.query.clone(),
            headers: req.headers.clone(),
            body: req.body.clone(),
        };
        let reply = author.dispatch(&inner_req);
        return metal_web::reply(&mut stream, reply.status_line(), reply.kind, &reply.body);
    }

    // A picture route: loopback callers may take QOI/raw instead of JPEG, and
    // two asks inside one frame time share the encoded answer.
    let mut query = req.query.clone();
    let cache_key = (req.method == "GET" && call::frame_cache::is_image_route(&req.path, &query)).then(|| {
        let loopback = stream.peer_addr().is_ok_and(|a| a.ip().is_loopback());
        call::frame_cache::negotiate(&mut query, loopback, req.headers.get("accept").map(String::as_str));
        call::frame_cache::key(&req.path, &query)
    });
    static FRAMES: Mutex<Option<call::frame_cache::Cache>> = Mutex::new(None);
    if let Some(key) = &cache_key {
        let hit = FRAMES.lock().ok().and_then(|c| c.as_ref()?.get(key, std::time::Instant::now()));
        if let Some((kind, body)) = hit {
            return metal_web::reply(&mut stream, "200 OK", kind, &body);
        }
    }
    let params = call::Params {
        query,
        body: req.body.clone(),
        tail: None,
    };
    // In-process tools (`POST /agent/tool`) run before the state lock, so a
    // slow one (`sleep`, a bake) never stalls the shell or other requests.
    let authorized = call::bearer_authorized(req.headers.get("authorization").map(String::as_str));
    if let Some(reply) = call::agent_intercept(&req.method, &req.path, &params, authorized) {
        return metal_web::reply(&mut stream, reply.status_line(), reply.kind, &reply.body);
    }
    let asked = web_time::Instant::now();
    let mut guard = shared.lock().expect("shared lock poisoned");
    let wait = asked.elapsed();
    let Shared { state, paint, sim } = &mut *guard;
    let mut ctx = call::Ctx {
        state,
        paint: paint.as_mut(),
        sim,
        author,
    };
    let reply = call::dispatch_deferred(&req.method, &req.path, &params, &mut ctx);
    drop(guard);
    perf::call(&req.method, &req.path, wait, asked.elapsed() - wait);
    // The picture encode runs here, after the lock drop.
    let reply = reply.map(|(reply, pending)| call::finish(reply, pending));
    if let (Some(key), Some(reply)) = (cache_key, &reply) {
        if reply.status == 200 {
            if let Ok(mut c) = FRAMES.lock() {
                c.get_or_insert_with(Default::default)
                    .put(key, reply.kind, reply.body.clone(), std::time::Instant::now());
            }
        }
    }
    match reply {
        Some(reply) => metal_web::reply(&mut stream, reply.status_line(), reply.kind, &reply.body),
        None => metal_web::not_found(&mut stream),
    }
}
/// Every layer currently routed to `device`, given fresh `outputs` of just
/// that one device — what "apply a collection" and "select a collection from
/// the mode dropdown" both hand a device: a copy, not a reference, per
/// [`Collection`]'s own note on why applying copies.
#[cfg(test)]
mod tests {
    use super::*;
    // The app itself reaches the digest through `Input` now; only the tests
    // still want a bare deterministic source, which is the reason `Simulated`
    // outlived the arrival of the real one.
    use digest::{Simulated, Source};
    // Only the colour-slot test reaches for a bare `Drive` (the no-GPU build
    // is where it runs alone); the app itself goes through `State`.
    use drive::Drive;
    use metal_schema::Block;

    #[test]
    fn named_floats_labels_every_own_float_and_collapses_the_shared_blocks() {
        // Through the painter rather than `metal_render::registry`, so this runs
        // on the GPU-free build too — which is the one where a manifest that
        // had lost a float would show up.
        let paint = paint::painter();
        for (id, schema) in paint.ids().into_iter().zip(paint.schemas()) {
            let row = schema.defaults();
            let named = named_floats(schema, &row);

            let own: usize = schema
                .blocks
                .iter()
                .map(|b| if let Block::Floats(f) = b { f.len() } else { 1 })
                .sum();
            assert_eq!(named.len(), own, "{id}: monitor row is the wrong length");

            for entry in &named {
                assert!(
                    entry["name"].as_str().is_some_and(|s| !s.is_empty()),
                    "{id}: an unnamed entry reached the monitor"
                );
                if let Some(unit) = entry["unit"].as_f64() {
                    assert!((0.0..=1.0).contains(&unit), "{id}: bar out of range");
                }
            }
        }
    }

    #[test]
    fn the_assets_list_marks_exactly_the_unbuilt_ones_unbuilt() {
        // The greyed cards are refused by `/select` on an index bound. If the
        // list and the bound ever disagreed, either a real model would be
        // unselectable or a nonexistent one would be selectable and panic the
        // renderer.
        let paint = paint::painter();
        let list = asset_list(paint.as_ref());
        assert_eq!(list.len(), paint.model_count() + UNBUILT.len());
        for (i, entry) in list.iter().enumerate() {
            assert_eq!(
                entry["built"].as_bool(),
                Some(i < paint.model_count()),
                "entry {i} disagrees with the /select bound"
            );
        }
    }

    #[test]
    fn a_colour_slot_reaches_the_monitor_as_four_hex_colours() {
        let paint = paint::painter();
        let mut sim = Simulated::new(1);
        let frame = sim.poll(3.0);
        for (index, schema) in paint.schemas().iter().enumerate() {
            if !schema
                .blocks
                .iter()
                .any(|b| matches!(b, Block::ColorSlot(..)))
            {
                continue;
            }
            let row = drive::row(schema, &schema.defaults(), &frame, &Drive::default());
            let named = named_floats(schema, &row);
            let slots: Vec<&Value> = named
                .iter()
                .filter(|e| e.get("colours").is_some())
                .collect();
            assert!(!slots.is_empty(), "model {index} lost its colour slot");
            for slot in slots {
                let colours = slot["colours"].as_array().expect("colours");
                assert_eq!(colours.len(), 4);
                for c in colours {
                    let hex = c.as_str().expect("hex string");
                    assert_eq!(hex.len(), 7, "not a hex colour: {hex}");
                    assert!(u32::from_str_radix(&hex[1..], 16).is_ok(), "not hex: {hex}");
                }
            }
        }
    }

    /// B6: a fresh install (a `data/` with `collections.json` but no
    /// `graphs/`) converts on first touch and leaves the original file
    /// alone, byte for byte.
    #[test]
    fn migrate_writes_graphs_and_leaves_collections_json_in_place() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-migrate-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");

        // The fixture is copied into the scratch dir; the repo's data/ is never read.
        let collections =
            metal_vfs::read_to_string(crate::test_fixtures::dir().join("collections.json"))
                .expect("fixture collections.json readable");
        let schema = metal_vfs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
        )
        .expect("repo's data/schema.json readable");
        metal_vfs::write(dir.join("collections.json"), &collections).unwrap();
        metal_vfs::write(dir.join("schema.json"), &schema).unwrap();

        migrate_collections_to_graphs_at(&dir);

        assert!(
            metal_vfs::is_dir(&dir.join("graphs")),
            "migrate did not create data/graphs/"
        );
        let entries: Vec<_> = metal_vfs::read_dir(dir.join("graphs"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            !entries.is_empty(),
            "migrate created graphs/ but wrote nothing into it"
        );
        assert!(
            entries
                .iter()
                .any(|e| e.path().extension().map(|x| x == "json").unwrap_or(false)),
            "no .json graph file was written"
        );

        let after = metal_vfs::read_to_string(dir.join("collections.json")).unwrap();
        assert_eq!(
            after, collections,
            "collections.json must be left byte-identical"
        );

        // Idempotent: a second run with graphs/ already present must not
        // touch anything (the "already done" marker from the doc comment).
        metal_vfs::remove_file(
            metal_vfs::read_dir(dir.join("graphs"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        migrate_collections_to_graphs_at(&dir);
        let entries_after: Vec<_> = metal_vfs::read_dir(dir.join("graphs"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries_after.len() < entries.len(),
            "migrate re-ran even though data/graphs/ already existed"
        );

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// `R22.5b`: the device-mode migration writes each mode onto the
    /// `output.device` node's own config, converts a legacy `stack.mode` node
    /// in place rather than leaving a second source, never duplicates on a
    /// second boot, retires any `compose.*`/`stack.mode` for `Disabled`, and
    /// never overwrites a mode an authored graph already had.
    #[test]
    fn device_modes_migrate_onto_the_output_node_once_and_never_overwrite() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-device-mode-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");

        let modes = vec![
            ("preview".to_string(), layers::DeviceMode::Automix),
            (
                "backwall".to_string(),
                layers::DeviceMode::Collection("noche".to_string()),
            ),
            ("off".to_string(), layers::DeviceMode::Disabled),
        ];
        // A device from before R22.5b: its graph carries the legacy node.
        let mut legacy = metal_graph::Graph::new(
            &device_graph_id("legacy"),
            "legacy",
            metal_graph::Domain::Live,
        );
        legacy.add_node("out", "output.device", [800.0, 0.0]);
        let legacy_node = legacy.add_node("mode", metal_graph::STACK_MODE_KIND, [-200.0, 0.0]);
        legacy.node_mut(&legacy_node).unwrap().config =
            metal_graph::Config::from_json_str(&layers::DeviceMode::Automix.describe().to_string())
                .unwrap();
        save_graph_at(&dir, &legacy).unwrap();
        let mut with_legacy = modes.clone();
        with_legacy.push(("legacy".to_string(), layers::DeviceMode::Own));

        migrate_device_modes_to_graphs_at(&dir, &with_legacy);

        for (id, mode) in &with_legacy {
            let g = load_graph_at(&dir, &device_graph_id(id)).expect("graph written");
            // Every mode survives migration except `Disabled`, which the
            // migration deliberately leaves as "no mode declared" (a device
            // that never had one is already off) — the field's fallback reads
            // it. The dropdown's own `set_graph_device_mode` does write
            // `disabled` explicitly (see the round-trip test).
            let want = match mode {
                layers::DeviceMode::Disabled => None,
                other => Some(other.clone()),
            };
            assert_eq!(
                graph_device_mode(&g),
                want,
                "{id} did not survive migration"
            );
            // No compose/stack node survives: the mode lives on the output.
            assert!(
                g.nodes
                    .iter()
                    .all(|n| !metal_graph::is_compose_kind(&n.kind)
                        && n.kind != metal_graph::STACK_MODE_KIND),
                "{id} kept a legacy mode node"
            );
            // The canvas `POST /outputs {op:"add"}` writes is still there.
            assert!(g.nodes.iter().any(|n| n.kind == "output.device"));
        }

        // Idempotent: a second run changes nothing.
        let before: Vec<String> = with_legacy
            .iter()
            .map(|(id, _)| {
                let g = load_graph_at(&dir, &device_graph_id(id)).unwrap();
                format!("{id}:{:?}", graph_device_mode(&g))
            })
            .collect();
        migrate_device_modes_to_graphs_at(&dir, &with_legacy);
        let after: Vec<String> = with_legacy
            .iter()
            .map(|(id, _)| {
                let g = load_graph_at(&dir, &device_graph_id(id)).unwrap();
                format!("{id}:{:?}", graph_device_mode(&g))
            })
            .collect();
        assert_eq!(before, after, "a second migration changed something");

        // The legacy node became the compose kind, same id — not a second
        // node beside it.
        let g = load_graph_at(&dir, &device_graph_id("legacy")).unwrap();
        assert!(g
            .nodes
            .iter()
            .all(|n| n.kind != metal_graph::STACK_MODE_KIND));
        assert!(
            g.nodes
                .iter()
                .all(|n| !metal_graph::is_compose_kind(&n.kind)),
            "the legacy stack node became output config, not a compose node"
        );
        assert_eq!(
            graph_device_mode(&g),
            Some(layers::DeviceMode::Own),
            "the legacy device's mode was read and written as own"
        );

        // The graph wins: a mode an operator set through the canvas is not
        // overwritten by the file's older value.
        let mut g = load_graph_at(&dir, &device_graph_id("preview")).unwrap();
        crate::set_graph_device_mode(&mut g, &layers::DeviceMode::Own).unwrap();
        save_graph_at(&dir, &g).unwrap();
        migrate_device_modes_to_graphs_at(&dir, &with_legacy);
        let g = load_graph_at(&dir, &device_graph_id("preview")).unwrap();
        assert_eq!(
            graph_device_mode(&g),
            Some(layers::DeviceMode::Own),
            "migration must not overwrite a mode the graph already declares"
        );

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// LAY.7b's own acceptance, one half: a device that was migrated and a
    /// device that was never migrated (the field is the fallback) resolve the
    /// *same mode* for the same file state, and a mode the dropdown wrote
    /// through [`write_device_mode_at`] reads back off the graph.
    #[test]
    fn a_migrated_device_and_a_fresh_one_resolve_the_same_mode() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-device-mode-parity-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");

        // Fresh: no graph at all — the field stands.
        let fresh = layers::Device {
            mode: layers::DeviceMode::Collection("noche".into()),
            ..Default::default()
        };
        let fresh_mode = device_graph_at(&dir, "fresh")
            .as_ref()
            .and_then(graph_device_mode)
            .unwrap_or_else(|| fresh.mode.clone());
        assert_eq!(fresh_mode, layers::DeviceMode::Collection("noche".into()));

        // No graph, no write: the route must not mint a canvas a caller did
        // not ask for — the field is the fallback and the boot migration is
        // what materializes it.
        write_device_mode_at(&dir, "fresh", &layers::DeviceMode::Own).unwrap();
        assert!(
            load_graph_at(&dir, &device_graph_id("fresh")).is_err(),
            "a device with no graph must not grow one from a dropdown write"
        );

        // Migrated: the graph declares it, and the graph wins even when the
        // field says something older.
        ensure_device_graph_at(&dir, "fresh", "fresh");
        write_device_mode_at(
            &dir,
            "fresh",
            &layers::DeviceMode::Collection("noche".into()),
        )
        .unwrap();
        let graph_mode = device_graph_at(&dir, "fresh")
            .as_ref()
            .and_then(graph_device_mode)
            .unwrap_or_else(|| fresh.mode.clone());
        assert_eq!(graph_mode, fresh_mode);

        // And a mode the dropdown writes afterwards is what reads back, not
        // the file's older value.
        write_device_mode_at(&dir, "fresh", &layers::DeviceMode::Own).unwrap();
        let after = device_graph_at(&dir, "fresh")
            .as_ref()
            .and_then(graph_device_mode)
            .unwrap_or_else(|| fresh.mode.clone());
        assert_eq!(after, layers::DeviceMode::Own);

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// §16.4: `GET /catalog` answers, and answers the whole catalog.
    ///
    /// The counts are asserted by *family* rather than as one total, because
    /// the page groups by family and a family that silently emptied would
    /// render as a heading with nothing under it — which reads as "this build
    /// has none of those" rather than as the bug it would be. `metal-graph`'s own
    /// `catalog_matches_schema` already guards the generators against
    /// `data/schema.json` drifting; this guards the route that serves them,
    /// which is a different thing and could break on its own (a `data_root`
    /// with no `schema.json`, for one).
    #[test]
    fn the_catalog_route_serves_every_family() {
        use metal_graph::NodeFamily;
        let catalog = node_catalog().expect("the repo's data/schema.json is readable and parses");

        let count = |f: NodeFamily| catalog.iter().filter(|k| k.family == f).count();
        // Counted from the manifest, not written down (rule 7): the literal
        // that stood here went stale the day `forge` was appended.
        let manifest: metal_schema::Manifest = serde_json::from_str(
            &metal_vfs::read_to_string(crate::output::data_root().join("schema.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            count(NodeFamily::Generator),
            manifest.models.iter().filter(|m| !m.footage).count() + 1,
            "one gen.* per non-footage model in data/schema.json, plus the gen.ramp source"
        );
        assert_eq!(
            count(NodeFamily::Effect),
            metal_schema::EFFECT_NAMES.len()
                + metal_schema::ADVANCED_EFFECT_IDS.len()
                + 1
                + metal_schema::TEXT_EFFECT_NAMES.len()
                + 2
                + 2
                + 2
                + metal_graph::PARITY_EFFECT_IDS.len(),
            "the common effects + the advanced effects, fx.forge, plus \
             the S.W per-glyph vocabulary, the K motion pair (fx.speed/fx.move), B's media \
             pair (fx.particles/fx.image-pop) and the loops pair (fx.warp/fx.flash)"
        );
        // The twelve fixed kinds, one family at a time — spelled out rather
        // than summed, so a kind lost from one family cannot be hidden by a
        // kind gained in another.
        assert_eq!(
            count(NodeFamily::Audio),
            3,
            "audio.live, audio.file and clock.timecode (P-D)"
        );
        assert_eq!(count(NodeFamily::Reel), 2, "reel.clip and reel.batch");
        assert_eq!(count(NodeFamily::Still), 1);
        assert_eq!(count(NodeFamily::Text), 1);
        assert_eq!(count(NodeFamily::Capture), 1);
        assert_eq!(count(NodeFamily::Router), 1);
        assert_eq!(
            count(NodeFamily::Script),
            3 + metal_graph::SCRIPT_BEAT_DIVISIONS.len() + 2,
            "script.py/rs/js (O.6, S.G) + the beat presets + fader + ctl.math"
        );
        assert_eq!(count(NodeFamily::Mask), 1);
        assert_eq!(count(NodeFamily::OutputDevice), 1);
        // `out.bluetooth` arrived with S.O.3 and this number stayed at 6. The
        // assertion did its job — it went red the moment the family grew — but
        // the workspace test build did not compile at the time, so nobody saw
        // it. Keeping the number *and* the names is the point: a bare count
        // says "something changed", the names say what was supposed to be here.
        assert_eq!(
            count(NodeFamily::OutputSignal),
            9,
            "the destinations that are not screens: ddp, json, sacn, osc, http, midi, bluetooth, serial, artnet"
        );
        assert_eq!(count(NodeFamily::ExportVideo), 1);
        assert_eq!(count(NodeFamily::Sensor), 1);
        assert_eq!(count(NodeFamily::CameraTrack), 1);
        assert_eq!(
            count(NodeFamily::Compose),
            3,
            "compose.automix/collection/layers — the bridge table (LAY.7b)"
        );
        assert_eq!(
            count(NodeFamily::Stack),
            1,
            "stack.mode — the legacy mode node the compose family superseded (O.4)"
        );
        assert_eq!(
            count(NodeFamily::Colour),
            2,
            "color and palette — the sources of control (K)"
        );
        assert_eq!(
            count(NodeFamily::Ai),
            1,
            "ai.colourist — the trained reaction (S.AM, wave 1)"
        );
        assert_eq!(
            count(NodeFamily::Input),
            1,
            "in.artnet — the Art-Net receiver (P-D)"
        );
        assert_eq!(
            count(NodeFamily::Math),
            metal_graph::source::KINDS.len(),
            "source.osc/const/random — the value nodes (M1, counted from the table)"
        );
        assert_eq!(
            count(NodeFamily::Venue),
            3,
            "venue.surface (P1), venue.fixtures (M3) and venue.fixture (F1) — the venue family"
        );
        assert_eq!(
            count(NodeFamily::Material),
            metal_graph::materials::KINDS.len(),
            "the mat.* surface nodes — counted from the table (materials brief M4)"
        );

        // Every kind lands in one of the families named above.
        //
        // **Summed, not a literal.** A hand-written total here is the exact
        // thing PARTE IV forbids, and this one had already gone stale twice by
        // the time O.4 added a family. The sum still catches what the literal
        // was for — a family added to `NodeFamily` that nobody counted above
        // makes it come out short — without needing to be edited every time a
        // model lands.
        let named: usize = [
            NodeFamily::Generator,
            NodeFamily::Effect,
            NodeFamily::Audio,
            NodeFamily::Reel,
            NodeFamily::Still,
            NodeFamily::Text,
            NodeFamily::Capture,
            NodeFamily::Router,
            NodeFamily::Script,
            NodeFamily::Mask,
            NodeFamily::Venue,
            NodeFamily::OutputDevice,
            NodeFamily::OutputSignal,
            NodeFamily::ExportVideo,
            NodeFamily::Sensor,
            NodeFamily::CameraTrack,
            NodeFamily::Compose,
            NodeFamily::Stack,
            NodeFamily::Colour,
            NodeFamily::Ai,
            NodeFamily::Input,
            NodeFamily::Math,
            NodeFamily::Material,
        ]
        .into_iter()
        .map(count)
        .sum();
        assert_eq!(
            named,
            catalog.len(),
            "a family exists that this test does not count — add it above"
        );

        // A kind is the string the page keys everything off, so no two may
        // collide — two entries called `gen.turing` would draw twice in the
        // menu and the second would shadow the first's model index.
        let mut kinds: Vec<&str> = catalog.iter().map(|k| k.kind.as_str()).collect();
        kinds.sort_unstable();
        let before = kinds.len();
        kinds.dedup();
        assert_eq!(
            kinds.len(),
            before,
            "two catalog entries share a kind string"
        );

        // **The join the "+ source" menu is built on.** A `gen.<id>` entry is
        // offered as creatable only because `POST /layers` can be handed the
        // index of that model in the built list — the page finds it by
        // matching `/state`'s `assets[i].id` against the part after `gen.`.
        // If a generator in the catalog were not a built model, that lookup
        // would miss and the menu would either offer a dead entry or, worse,
        // fall back to index 0 and create a source showing the wrong
        // animation: silent, in range, and wrong (§3.3, §11.7). Asserted
        // here rather than trusted, because the two lists are built by
        // different code — the catalog from `data/schema.json`, the built
        // models from the compiled renderers.
        let paint = paint::painter();
        let built = paint.ids();
        // `gen.ramp` is the one generator that is not a model (rule 1): the
        // painter rasterises it before any model lookup (`Paint::run`).
        for kind in catalog
            .iter()
            .filter(|k| k.family == NodeFamily::Generator && k.kind != metal_graph::RAMP_KIND)
        {
            let id = kind
                .kind
                .strip_prefix("gen.")
                .expect("a generator kind is `gen.<model_id>`");
            assert!(
                built.contains(&id),
                "catalog offers gen.{id} but no built model has that id — the menu would \
                 create a source pointed at the wrong animation"
            );
        }
    }

    // -----------------------------------------------------------------
    // O.4: `mode` as a graph-level construct
    // -----------------------------------------------------------------

    /// A Live graph carrying one `stack.mode` node with `config`.
    fn graph_with_mode(config: Option<serde_json::Value>) -> metal_graph::Graph {
        let mut g = metal_graph::Graph::new("device-test", "test", metal_graph::Domain::Live);
        let id = g.add_node("mode", metal_graph::STACK_MODE_KIND, [0.0, 0.0]);
        if let (Some(cfg), Some(n)) = (config, g.node_mut(&id)) {
            n.config = metal_graph::Config::from_json_str(&cfg.to_string()).expect("valid json");
        }
        g
    }

    /// A Live graph carrying one `compose.*` node of `kind` with `config`.
    fn graph_with_compose(kind: &str, config: Option<serde_json::Value>) -> metal_graph::Graph {
        let mut g = metal_graph::Graph::new("device-test", "test", metal_graph::Domain::Live);
        let id = g.add_node("comp", kind, [0.0, 0.0]);
        if let (Some(cfg), Some(n)) = (config, g.node_mut(&id)) {
            n.config = metal_graph::Config::from_json_str(&cfg.to_string()).expect("valid json");
        }
        g
    }

    /// A Live graph with the one `output.device` node `R22.5b`'s mode lives
    /// on — no legacy composition node.
    fn graph_with_output(id: &str) -> metal_graph::Graph {
        let mut g = metal_graph::Graph::new(id, "test", metal_graph::Domain::Live);
        g.add_node("out", "output.device", [800.0, 0.0]);
        g
    }

    #[test]
    fn a_graph_with_no_composition_node_says_nothing_and_the_dropdown_stands() {
        // Every device authored before O.4. The fallback is what keeps this
        // change additive.
        let g = metal_graph::Graph::new("device-test", "test", metal_graph::Domain::Live);
        assert_eq!(graph_device_mode(&g), None);
    }

    #[test]
    fn every_device_mode_round_trips_through_a_compose_node() {
        // LAY.7b's legacy vocabulary: a `compose.*` kind still reads back as
        // its mode (the migration has not touched this graph). R22.5b's own
        // round-trip — the writer the dropdown uses — is the second half.
        for (kind, cfg, want) in [
            ("compose.automix", None, layers::DeviceMode::Automix),
            ("compose.layers", None, layers::DeviceMode::Own),
            (
                "compose.collection",
                Some(serde_json::json!({ "id": "noche-1" })),
                layers::DeviceMode::Collection("noche-1".into()),
            ),
        ] {
            let g = graph_with_compose(kind, cfg);
            assert_eq!(
                graph_device_mode(&g),
                Some(want.clone()),
                "{kind} did not read back as {want:?}"
            );
        }
        // The writer and the reader are one vocabulary: every mode the
        // dropdown can write lands on the `output.device` node and reads back
        // as itself, however the graph started.
        for mode in [
            layers::DeviceMode::Automix,
            layers::DeviceMode::Own,
            layers::DeviceMode::Collection("noche-1".into()),
            layers::DeviceMode::Disabled,
        ] {
            let mut g = graph_with_output("device-test");
            set_graph_device_mode(&mut g, &mode).unwrap();
            assert_eq!(graph_device_mode(&g), Some(mode.clone()), "{mode:?}");
            // And no compose/stack node survived the write.
            assert!(
                g.nodes
                    .iter()
                    .all(|n| !metal_graph::is_compose_kind(&n.kind)
                        && n.kind != metal_graph::STACK_MODE_KIND),
                "{mode:?} left a legacy mode node behind"
            );
        }
    }

    #[test]
    fn a_compose_node_wins_over_a_legacy_stack_node_when_both_are_present() {
        // The reader's order, asserted: `compose.*` first. (A graph with both
        // is the ambiguous state `lint` refuses; this is what makes the
        // reader's answer the same one the migration would leave behind.)
        let mut g = graph_with_mode(Some(layers::DeviceMode::Automix.describe()));
        let id = g.add_node("comp", "compose.layers", [0.0, 0.0]);
        let _ = id;
        assert_eq!(graph_device_mode(&g), Some(layers::DeviceMode::Own));
    }

    #[test]
    fn a_compose_collection_with_no_id_falls_back_rather_than_naming_nothing() {
        // Same posture as the legacy reader: a malformed config lands on the
        // operator's own dropdown, which is never wrong, only older.
        assert_eq!(
            graph_device_mode(&graph_with_compose("compose.collection", None)),
            None
        );
        assert_eq!(
            graph_device_mode(&graph_with_compose(
                "compose.collection",
                Some(serde_json::json!({ "id": 7 }))
            )),
            None,
            "a non-string id names nothing"
        );
        // A legacy node's unreadable config is still read the same way.
        assert_eq!(graph_device_mode(&graph_with_mode(None)), None);
    }

    #[test]
    fn every_device_mode_round_trips_through_a_stack_node() {
        // The property that makes this one vocabulary and not two: whatever
        // `POST /device` writes, the node can carry, and it comes back equal.
        for mode in [
            layers::DeviceMode::Automix,
            layers::DeviceMode::Disabled,
            layers::DeviceMode::Own,
            layers::DeviceMode::Collection("noche-1".into()),
        ] {
            let g = graph_with_mode(Some(mode.describe()));
            assert_eq!(
                graph_device_mode(&g),
                Some(mode.clone()),
                "{mode:?} did not survive the graph"
            );
        }
    }

    #[test]
    fn a_stack_node_with_an_unreadable_config_falls_back_rather_than_blanking_the_screen() {
        // Same posture `NodeLayerConfig::from_node` takes, and it matters
        // more here: falling back lands on the operator's own dropdown, which
        // is never wrong, only older. Guessing would turn a hand-edited file
        // into a dark projector.
        assert_eq!(
            graph_device_mode(&graph_with_mode(None)),
            None,
            "no config at all"
        );
        assert_eq!(
            graph_device_mode(&graph_with_mode(Some(
                serde_json::json!({ "kind": "telepatia" })
            ))),
            None,
            "a mode this build has never heard of"
        );
        assert_eq!(
            graph_device_mode(&graph_with_mode(Some(
                serde_json::json!({ "kind": "collection" })
            ))),
            None,
            "a collection mode with no id names nothing"
        );
    }

    #[test]
    fn a_mode_node_is_not_reported_as_an_unconnected_node() {
        // `stack.mode` and every `compose.*` kind have no ports at all — see
        // `metal_graph::NodeFamily::Stack`/`Compose`. Warning about them would
        // put a permanent complaint on every graph that has one, which is how
        // an operator learns to stop reading warnings.
        let catalog = node_catalog().expect("the repo's data/schema.json parses");
        for g in [
            graph_with_mode(Some(layers::DeviceMode::Automix.describe())),
            graph_with_compose("compose.automix", None),
            graph_with_compose(
                "compose.collection",
                Some(serde_json::json!({ "id": "noche" })),
            ),
        ] {
            let issues = g.lint(&catalog);
            assert!(
                !issues.iter().any(|i| i.message.contains("not connected")),
                "{issues:?}"
            );
        }
    }

    /// H.8: `GET /ai-config` on a fresh `data/` (no `ai.json` yet) answers
    /// `{"configured": false}` rather than an error — matching
    /// `ProviderConfig::load`'s own "missing file is `None`, not `Err`"
    /// contract one layer up.
    #[test]
    fn ai_config_get_reports_unconfigured_when_no_file_exists() {
        let dir = scratch_dir("ai-config-unconfigured");
        let reply = ai_config_get_reply_at(dir.join("ai.json"), &dir.join("absent-config.toml"))
            .expect("no file is not an error");
        assert_eq!(reply, json!({ "configured": false }));
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// H.8's core rule: `POST /ai-config` accepts a raw key and writes it to
    /// disk, but every reply this route ever sends back — including the one
    /// immediately after the save that just accepted it — carries only the
    /// masked tail. A key that round-trips into the reply body is a key that
    /// would round-trip into the browser's own JS state and, from there, a
    /// stray `console.log`.
    #[test]
    fn ai_config_post_never_echoes_the_raw_key_back() {
        let dir = scratch_dir("ai-config-post");
        let path = dir.join("ai.json");
        let body = json!({
            "preset": "claude",
            "url": "https://api.anthropic.com/v1/messages",
            "model": "claude-sonnet-5",
            "key": "sk-supersecretvalue9999",
        });
        let reply = ai_config_post_at(&path, &dir.join("absent-config.toml"), &body)
            .expect("valid body saves");
        let rendered = reply.to_string();
        assert!(
            !rendered.contains("sk-supersecretvalue9999"),
            "raw key leaked into the reply"
        );
        assert_eq!(reply["key_masked"], "***9999");
        assert_eq!(reply["configured"], true);
        assert_eq!(reply["preset"], "claude");

        // And the file on disk actually has the real key — `POST` writing a
        // masked value would silently break the next real request the loop
        // makes with this config.
        let saved =
            metal_agent::config::ProviderConfig::load_at(&path, &dir.join("absent-config.toml"))
                .unwrap()
                .unwrap();
        assert_eq!(saved.key, "sk-supersecretvalue9999");

        // A follow-up `GET` — the page reloading the overlay later — must
        // still only ever see the masked form, never the raw key.
        let get_reply = ai_config_get_reply_at(&path, &dir.join("absent-config.toml"))
            .expect("saved file reads back");
        assert_eq!(get_reply["key_masked"], "***9999");
        assert!(!get_reply.to_string().contains("sk-supersecretvalue9999"));

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The masked-placeholder half of H.8: leaving `key` out of the body (the
    /// page's own behaviour when the operator hasn't retyped it) keeps
    /// whatever key is already on disk instead of erroring or blanking it.
    #[test]
    fn ai_config_post_keeps_existing_key_when_none_is_sent() {
        let dir = scratch_dir("ai-config-keep-key");
        let path = dir.join("ai.json");
        metal_agent::config::ProviderConfig::new(
            metal_agent::config::Preset::Openai,
            "gpt-5",
            "sk-original-key-0000",
        )
        .save_at(&path, &dir.join("absent-config.toml"))
        .unwrap();

        let body = json!({
            "preset": "openai",
            "url": "https://api.openai.com/v1/chat/completions",
            "model": "gpt-5-mini",
        });
        let reply = ai_config_post_at(&path, &dir.join("absent-config.toml"), &body)
            .expect("omitted key keeps the old one");
        assert_eq!(
            reply["model"], "gpt-5-mini",
            "the rest of the config still updates"
        );

        let saved =
            metal_agent::config::ProviderConfig::load_at(&path, &dir.join("absent-config.toml"))
                .unwrap()
                .unwrap();
        assert_eq!(
            saved.key, "sk-original-key-0000",
            "key must survive an update that omits it"
        );

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// A `POST` with no key and nothing on disk yet has no key to keep — this
    /// must be a 400, not a config saved with an empty key that then fails
    /// silently the first time the loop tries to authenticate with it.
    #[test]
    fn ai_config_post_rejects_missing_key_on_first_save() {
        let dir = scratch_dir("ai-config-no-key-yet");
        let body = json!({
            "preset": "manual",
            "url": "https://example.test/v1",
            "model": "whatever",
        });
        let err = ai_config_post_at(dir.join("ai.json"), &dir.join("absent-config.toml"), &body)
            .unwrap_err();
        assert!(
            err.contains("key"),
            "error should say a key was expected: {err}"
        );
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// `S.U14`: `POST /ai/ask` appends and `GET /ai/log`
    /// (`ai_log_load`) reads the same file back, in order.
    #[test]
    fn ai_ask_appends_to_the_log_in_order() {
        let dir = scratch_dir("ai-ask-log");
        let path = dir.join("ai_log.json");
        ai_ask_post_at(&path, &json!({ "prompt": "primero" })).expect("first ask");
        ai_ask_post_at(&path, &json!({ "prompt": "segundo" })).expect("second ask");
        let log = ai_log_load(&path);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0]["content"], "primero");
        assert_eq!(log[1]["content"], "segundo");
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn ai_ask_rejects_an_empty_prompt() {
        let dir = scratch_dir("ai-ask-empty");
        let err = ai_ask_post_at(dir.join("ai_log.json"), &json!({ "prompt": "   " })).unwrap_err();
        assert!(err.contains("prompt"), "{err}");
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// A scratch `data/`-shaped dir for D1 tests, so `save_graph_at`/
    /// `load_graph_at` never touch the repo's real `data/graphs/` — same
    /// scheme `migrate_writes_graphs_and_leaves_collections_json_in_place`
    /// uses for the same reason.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-{name}-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// D1: a graph saved and reloaded is byte-for-byte the same graph — the
    /// round-trip `check-graph.mjs` is meant to hold the client to, proven
    /// first at the Rust layer that actually owns the file.
    #[test]
    fn a_saved_graph_reloads_identical() {
        let dir = scratch_dir("graph-roundtrip");
        let mut g = metal_graph::Graph::new("g1", "Test graph", metal_graph::Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [10.0, 20.0]);
        let out = g.add_node("out", "output.device", [300.0, 20.0]);
        g.add_edge(
            metal_graph::Endpoint::new(&gen, "image"),
            metal_graph::Endpoint::new(&out, "image"),
        );

        save_graph_at(&dir, &g).expect("save");
        let reloaded = load_graph_at(&dir, "g1").expect("load");
        assert_eq!(
            g, reloaded,
            "save/load must round-trip byte-identical, per §4's check-graph.mjs gate"
        );

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_a_graph_that_does_not_exist_names_the_id_not_a_raw_io_error() {
        let dir = scratch_dir("graph-missing");
        let err = load_graph_at(&dir, "does-not-exist").unwrap_err();
        assert!(
            err.contains("does-not-exist"),
            "error must name the missing graph: {err}"
        );
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// D1/E4: `add_edge_checked` is the door E3/E4's UI will hang the "off /
    /// reactivo / fijo" materialization off of — it has to actually reject a
    /// cycle, not just paint a warning after the fact.
    #[test]
    fn add_edge_checked_accepts_a_valid_edge() {
        let catalog = node_catalog().expect("repo schema.json parses");
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        // `fx.*`, not `output.device`: an output device also requires a
        // control (digest) edge before lint is clean (see
        // `output_device_without_control_is_an_error` in `metal-graph`), which
        // this test is not about — a generator feeding an effect is a
        // complete, valid edge on its own.
        let fx = g.add_node("fx", "fx.shake", [200.0, 0.0]);
        let result = add_edge_checked(
            &mut g,
            metal_graph::Endpoint::new(&gen, "image"),
            metal_graph::Endpoint::new(&fx, "image"),
            &catalog,
        );
        assert!(
            result.is_ok(),
            "a plain gen -> fx edge must be accepted: {result:?}"
        );
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn add_edge_checked_rejects_a_cycle_and_rolls_back() {
        let catalog = node_catalog().expect("repo schema.json parses");
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let fx1 = g.add_node("fx", "fx.shake", [0.0, 0.0]);
        let fx2 = g.add_node("fx", "fx.crt", [1.0, 0.0]);
        g.add_edge(
            metal_graph::Endpoint::new(&fx1, "image"),
            metal_graph::Endpoint::new(&fx2, "image"),
        );
        let before = g.edges.len();

        // Wiring fx2's output back into fx1's input closes a cycle.
        let result = add_edge_checked(
            &mut g,
            metal_graph::Endpoint::new(&fx2, "image"),
            metal_graph::Endpoint::new(&fx1, "image"),
            &catalog,
        );
        assert!(
            result.is_err(),
            "a cycle must be rejected, not silently drawn"
        );
        assert_eq!(
            g.edges.len(),
            before,
            "the rejected edge must not stay on the graph"
        );
    }

    #[test]
    fn add_edge_checked_rejects_mismatched_port_types() {
        let catalog = node_catalog().expect("repo schema.json parses");
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let cam = g.add_node("cam", "cam.track", [200.0, 0.0]);
        // gen.turing's `image` output (PortType::Image) into cam.track's
        // `control` input (PortType::Control) — both ports exist, types
        // disagree.
        let result = add_edge_checked(
            &mut g,
            metal_graph::Endpoint::new(&gen, "image"),
            metal_graph::Endpoint::new(&cam, "control"),
            &catalog,
        );
        assert!(
            result.is_err(),
            "an image output wired into a control-typed input must be rejected"
        );
        assert!(g.edges.is_empty());
    }

    /// D3: `graph_reply` is what every `/graph*` route converges its
    /// response on — proving it directly means a route test never has to
    /// spin up a real socket to check that a saved graph comes back with
    /// primitives and issues attached.
    #[test]
    fn graph_reply_carries_primitives_and_issues() {
        let catalog = node_catalog().expect("repo schema.json parses");
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        // `add_node` answers the id it minted (`gen-0`), not the requested stem.
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]); // orphan: no output wired anywhere — a lint warning, not an error.

        let reply = graph_reply(&g, &catalog);
        assert_eq!(reply["graph"]["id"], json!("g"));
        let prims = reply["primitives"].as_array().unwrap();
        assert!(
            !prims.is_empty(),
            "one node must produce at least its box and label as primitives"
        );
        // N: every primitive is tagged — a painter never guesses a shape from
        // its size. A lone generator produces a `node` box, a `header` band, a
        // `title`, a port dot and at least one parameter row.
        assert!(
            prims.iter().any(|p| p["tag"] == json!("node")),
            "a node box is tagged `node`"
        );
        assert!(
            prims.iter().any(|p| p["tag"] == json!("header")),
            "the header band is tagged `header`"
        );
        assert!(
            prims.iter().any(|p| p["tag"] == json!("title")),
            "the node's title is tagged `title`"
        );
        assert!(
            prims
                .iter()
                .any(|p| p["tag"] == json!("port") && p["node"] == json!(gen)),
            "a port dot names its node"
        );
        assert!(
            prims.iter().any(|p| p["tag"] == json!("value_field")),
            "a generator's float rows carry value fields"
        );
        assert!(
            reply["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["severity"] == "warning"),
            "an unconnected generator is a lint warning — graph_reply must surface it"
        );
    }

    /// B4: the config panel's whole read for a generator — every effect the
    /// catalog offers present, the materialized one leading (chain order),
    /// the others filled in as `off` by `effects_reply` itself rather than by
    /// anything stored on the node. The count is the catalog's own, so a new
    /// effect (`fx.particles`/`fx.image-pop`, B's wave) does not need this
    /// test edited.
    #[test]
    fn effects_reply_lists_every_effect_with_the_present_one_first() {
        let catalog = node_catalog().expect("repo schema.json parses");
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "vignette",
            metal_schema::EffectState::Fixed(0.4),
            metal_graph::effects::ControlSource::RawIngest,
        )
        .expect("materializes");

        let reply = effects_reply(&g, &gen, &catalog);
        let effects = reply["effects"].as_array().expect("effects is an array");
        // Every catalog `fx.*` a generator's panel offers, minus the glyph
        // vocabulary and the three kinds that are not `Post` amounts
        // (`fx.forge`, `fx.speed`, `fx.move`) — computed from the catalog,
        // never a hand-written total (rule 7).
        let offered = catalog
            .iter()
            .filter(|k| {
                k.family == metal_graph::NodeFamily::Effect
                    && !k.kind.starts_with("fx.text.")
                    && k.kind != metal_graph::FORGE_EFFECT_KIND
                    && k.kind != format!("fx.{}", metal_graph::SPEED_EFFECT_ID)
                    && k.kind != format!("fx.{}", metal_graph::MOVE_EFFECT_ID)
            })
            .count();
        assert_eq!(
            effects.len(),
            offered,
            "one row per catalog effect a panel can switch"
        );
        assert_eq!(
            effects[0]["effect"],
            json!("vignette"),
            "the present effect must lead the list"
        );
        assert_eq!(effects[0]["state"], json!("fixed"));
        // Compared with tolerance, not `assert_eq!` against `json!(0.4)`:
        // the value crossed an f32 (`EffectState::Fixed`) -> f64 (`Value`)
        // boundary, and 0.4 has no exact binary representation in either —
        // `0.4_f32 as f64` and `0.4_f64` are two slightly different numbers.
        let value = effects[0]["value"].as_f64().expect("fijo carries a value");
        assert!((value - 0.4).abs() < 1e-6, "expected ~0.4, got {value}");
        assert!(
            effects[1..].iter().all(|e| e["state"] == json!("off")),
            "every effect nothing materialized must read off: {effects:?}"
        );
    }

    /// B5, through the HTTP layer's own helpers rather than `metal_graph::effects`
    /// directly: turning an effect on and back off round-trips through
    /// `effects_reply` with nothing left over — the same "no second truth"
    /// guarantee, proved at the door the canvas actually calls.
    #[test]
    fn effects_reply_reflects_reactivo_then_off_with_nothing_left_over() {
        let catalog = node_catalog().expect("repo schema.json parses");
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.add_node("audio", "audio.live", [-1.0, 0.0]);
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "shake",
            metal_schema::EffectState::Reactive,
            metal_graph::effects::ControlSource::RawIngest,
        )
        .expect("reactivo materializes and wires from the audio node present in the graph");

        let reply = effects_reply(&g, &gen, &catalog);
        let shake = reply["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["effect"] == json!("shake"))
            .expect("shake is listed");
        assert_eq!(shake["state"], json!("reactive"));
        assert_eq!(shake["source"], json!("raw"));

        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "shake",
            metal_schema::EffectState::Off,
            metal_graph::effects::ControlSource::RawIngest,
        )
        .expect("turns back off");
        let reply2 = effects_reply(&g, &gen, &catalog);
        let shake2 = reply2["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["effect"] == json!("shake"))
            .expect("shake is still listed, now off");
        assert_eq!(shake2["state"], json!("off"));
        assert_eq!(
            shake2["node"],
            Value::Null,
            "off must not point at a node that no longer exists"
        );
        assert!(
            g.nodes.iter().all(|n| n.kind != "fx.shake"),
            "the fx node itself must be gone, not just relabeled"
        );
    }

    // -----------------------------------------------------------------------
    // The fan-out fixture the real data/collections.json cannot exercise:
    // this repo's one real collection is a single full-frame layer, so
    // nothing in it shares a generator or disagrees about draw order. These
    // are built in-test, never written to disk — `data/collections.json`
    // stays untouched.
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // LAY.0b — the conformance corpus, widened. `render_frames_are_pixel_
    // identical_through_the_plan` above is the original single-fixture gate;
    // these are the same assertion over every shape a real device takes:
    // automix, a collection, an overlay, a scheduled run, contours, per-layer
    // effects, a remote layer and a ceded screen. The plan's own words: the
    // safety net LAY.7-LAY.10 may not land without. Every fixture is built
    // in-test; nothing here writes to disk.
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // the design notes PARTE IV §17.4 Conjunto E — per-layer effects.
    // -----------------------------------------------------------------------

    /// **The acceptance test Conjunto E exists for.** Before
    /// `Layer::effects`/`fijo_mask` and `apply_node_post`, every row's
    /// trailing effects floats were overwritten by `apply_master_post` from
    /// the one app-wide `state.post` — so two layers on the very same
    /// generator could never disagree about a filter, which is exactly what
    /// §17.4's cheap duplication ("twenty beans on rotations and distinct
    /// filters") needs and could not have. Built through the real render
    /// path — `row_for` then `apply_node_post`, the same two calls
    /// `render_device` makes per layer — rather than a
    /// row poked together by hand.
    #[test]
    fn two_layers_on_one_model_can_fix_shake_to_different_values_in_the_same_frame() {
        let mut paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);

        let mut fixed = Layer::new("fixed", "fixed", 0);
        fixed.effects.shake = 0.6;
        fixed.fijo_mask = 1 << 0; // bit 0 — shake, apply_node_post's own order
        fixed.outputs = vec![PREVIEW.to_string()];

        let mut reactive = Layer::new("reactive", "reactive", 0);
        reactive.outputs = vec![PREVIEW.to_string()];
        // `reactive.effects`/`fijo_mask` stay at `Layer::new`'s own
        // defaults — `Post::default()` and 0 — i.e. fully reactive.

        let layers = vec![fixed, reactive];
        state.layers = layers.clone();

        let time = state.time();
        let frame = sim.poll(time);
        let air = state.airing(&mut sim, time);
        let models = paint.model_count();
        let passage = state.router.content.passage();

        let mut shakes = Vec::new();
        for layer in &layers {
            let showing = layer.showing(&air, models);
            let base = state.base[showing].clone();
            let mut row = row_for(
                &mut *paint,
                &mut state,
                showing,
                &base,
                &frame,
                time,
                passage,
                layer.library.as_deref(),
            );
            apply_node_post(&mut row, &layer.effects, layer.fijo_mask);
            let at = row.len() - metal_schema::EFFECTS_WIDTH;
            shakes.push(row[at]);
        }

        assert_eq!(shakes[0], 0.6, "the fixed layer's own shake did not stick");
        assert_ne!(
            shakes[0], shakes[1],
            "two layers sharing a generator ended up with the same shake — \
             apply_node_post must let them disagree"
        );
    }

    /// The master dial keeps doing from outside exactly what it always did:
    /// `POST /drive` with a body that only names `shake` must still change
    /// what `GET /state` reports as the master value, and — per
    /// `apply_fields`'s own "only touch what came" rule — must not touch any
    /// layer's `light_leak`, which this body never mentioned. Exercises
    /// `apply_post_op` directly, the same function the route calls — this
    /// file has no fixture for driving the HTTP layer itself, same reason
    /// `apply_layers_op`'s own tests do not either.
    #[test]
    fn the_master_dial_still_fixes_every_layer_and_only_the_field_it_was_sent() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());
        let mut sim = Listeners::stand_in();

        state.layers = vec![Layer::new("a", "a", 0), Layer::new("b", "b", 1)];

        apply_post_op(&mut state, &mut sim, &json!({ "shake": 0.42 }));

        assert_eq!(
            state.post.shake, 0.42,
            "GET /state must still report the master value"
        );
        for layer in &state.layers {
            assert_eq!(
                layer.effects.shake, 0.42,
                "layer {} did not receive the master shake",
                layer.id
            );
            assert_eq!(
                layer.fijo_mask & (1 << 0),
                1 << 0,
                "layer {} did not have its shake bit fixed",
                layer.id
            );
            // "Only touch what came": a field this body never mentioned must
            // be untouched on every layer, value and bit both.
            assert_eq!(
                layer.effects.light_leak, 0.0,
                "layer {}'s light_leak moved though the body never sent it",
                layer.id
            );
            assert_eq!(
                layer.fijo_mask & (1 << 1),
                0,
                "layer {} had its light_leak bit set though the body never sent it",
                layer.id
            );
        }
    }

    // -----------------------------------------------------------------------
    // §16.7 — a saved movement playing over a live layer.
    //
    // `overlay.rs` proves the player itself (the tick maths, the model guard,
    // the off switch) with no renderer in sight. These three are about the
    // seam: that the row it computes actually reaches the pixels, that it
    // reaches *both* render paths identically, and that it does not corrupt
    // the generator dedup on the way.
    // -----------------------------------------------------------------------

    /// A row that is in range for a model and is not its resting row, built
    /// from the model's own declared maxima so it cannot be clamped back to
    /// where it started.
    fn extreme_row(schema: &metal_schema::Schema) -> Vec<f32> {
        schema
            .blocks
            .iter()
            .flat_map(|b| match b {
                metal_schema::Block::Floats(fs) => fs.iter().map(|s| s.max).collect::<Vec<f32>>(),
                metal_schema::Block::ColorSlot(_, start) => start.to_vec(),
                metal_schema::Block::Camera(view) => view.to_vec(),
            })
            .collect()
    }

    /// Arms a one-tick loop of `row` on one layer of the preview, at the
    /// frozen clock the fixtures use.
    fn arm(state: &mut State, layer: &str, model: usize, row: Vec<f32>) {
        state.overlays.start(overlay::Play {
            device: PREVIEW.to_string(),
            layer: layer.to_string(),
            movement: "fixture".into(),
            model,
            ticks: vec![row],
            started: state.time(),
            looping: true,
            mask: None,
        });
    }

    /// The regression the whole `Playing` parameter exists for.
    ///
    /// `back` and `echo` share a generator because they share model, feed and
    /// (absent) float row — that is the fan-out case above, and it is correct.
    /// Arm a movement on one of them and they no longer draw the same pixels,
    /// so folding them would mean one row winning for both and the trigger
    /// either doing nothing or firing on a layer nobody aimed at. Silent, in
    /// range, wrong. This is what fails if overlays are ever applied after the
    /// key is computed instead of before.
    #[test]
    fn an_overlay_stops_two_otherwise_identical_layers_sharing_one_generator() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.layers = fan_out_stack();
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);

        arm(&mut state, "back", 0, extreme_row(paint.schemas()[0]));

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
            state.overlays.on(PREVIEW, state.time()),
            &state.collections,
            // Nothing scheduled — none of these tests arm a `sequence::Run`.
            None,
            "laptop",
        )
        .expect("a non-empty stack must compile");

        let idx_of = |id: &str| list.iter().position(|l| l.id == id).unwrap();
        assert_ne!(
            node_of_layer[idx_of("back")],
            node_of_layer[idx_of("echo")],
            "a layer with a movement playing must not share a generator with one without"
        );
        // And the row actually landed on the layer the renderer reads, in the
        // field the four base-row sites already consult.
        assert!(
            list[idx_of("back")].base.is_some(),
            "the played row reached the layer"
        );
        assert_eq!(list[idx_of("echo")].base, None, "and only that layer");
        assert_eq!(list[idx_of("base")].base, None);
    }

    /// The played row has to reach the pixels: an overlay applied to the
    /// list but not to the frame would be a movement the operator can watch
    /// do nothing.
    #[test]
    fn a_movement_changes_the_frame() {
        let mut paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.layers = fan_out_stack();
        state.preview.composition = Composition::Overlap;
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);

        let (resting, w, h) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);

        let row = extreme_row(paint.schemas()[0]);
        assert_ne!(
            row,
            paint.schemas()[0].defaults(),
            "the fixture row must differ from the resting row or this proves nothing"
        );
        arm(&mut state, "back", 0, row);

        let (played, w2, h2) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
        assert_eq!((w, h), (w2, h2));
        assert_ne!(
            resting, played,
            "a movement playing must change what the device draws"
        );
    }

    /// G5: the source card's picture as a texture is the same picture the
    /// JPEG route encodes from `render_source` (the solo draw, the rows, the
    /// node post): read back, it matches the pixels to within the animation's
    /// own clock between the two calls.
    #[test]
    fn a_source_card_texture_is_the_picture_render_source_draws() {
        let mut paint = paint::painter();
        let Some(_) = paint.shared_gpu() else {
            eprintln!("skipped: the CPU painter draws no texture");
            return;
        };
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.layers = fan_out_stack();
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);
        let pixels = render_source(&mut *paint, &mut sim, &mut state, PREVIEW, "back", 32, 18)
            .expect("the layer exists");
        let frame =
            render_source_texture(&mut *paint, &mut sim, &mut state, PREVIEW, "back", 32, 18)
                .expect("a GPU painter keeps the source as a texture");
        assert_eq!((frame.width(), frame.height()), (pixels.1, pixels.2));
        let texture = paint.readback(&frame);
        let mean = texture
            .iter()
            .zip(&pixels.0)
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).unsigned_abs() as f64)
            .sum::<f64>()
            / texture.len() as f64;
        assert!(mean < 2.0, "mean difference {mean:.3} per byte");
    }

    /// The source card's own thumbnail draws the movement too.
    ///
    /// A trigger that changed the wall but not the picture the operator is
    /// looking at while they tap it reads as a trigger that did nothing —
    /// which is how an operator ends up firing it four more times.
    #[test]
    fn a_source_card_draws_the_movement_playing_on_it() {
        let mut paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.layers = fan_out_stack();
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);

        let resting = render_source(&mut *paint, &mut sim, &mut state, PREVIEW, "back", 32, 18)
            .expect("the layer exists");
        arm(&mut state, "back", 0, extreme_row(paint.schemas()[0]));
        let played = render_source(&mut *paint, &mut sim, &mut state, PREVIEW, "back", 32, 18)
            .expect("the layer exists");
        assert_ne!(
            resting.0, played.0,
            "the card must show what the layer is playing"
        );

        // The layer with nothing armed is untouched, which is what says the
        // lookup is by layer and not by device.
        let a = render_source(&mut *paint, &mut sim, &mut state, PREVIEW, "echo", 32, 18);
        arm(&mut state, "back", 0, paint.schemas()[0].defaults());
        let b = render_source(&mut *paint, &mut sim, &mut state, PREVIEW, "echo", 32, 18);
        assert_eq!(
            a, b,
            "a movement on one layer must not move another one's picture"
        );
    }

    /// Step 2's own gate: draw order, read off the plan-based path, still
    /// comes out back-to-front by z — including across a stack with a
    /// shared generator, where "the plan" could plausibly scramble two
    /// layers that resolve to the same node.
    #[test]
    fn draw_order_through_the_plan_still_goes_back_to_front_by_z() {
        let mut paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());

        state.layers = fan_out_stack();
        state.preview.composition = Composition::Overlap;
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);

        let air = state.airing(&mut sim, state.time());
        let manifest_ids = paint.ids();
        let (list, graph, plan, node_of_layer, _remote, _mode) = compile_device_plan(
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

        // `list` itself is `layers::layers_on`'s own output — back to front
        // by z — so this is the sequencing baseline everything else (the
        // plan's op order, the tiles handed to `compose::compose`) has to
        // agree with.
        let ids: Vec<&str> = list.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["base", "back", "echo"], "z order was not honoured");

        // The plan's own op order for the *distinct* nodes must not put the
        // shared pair's node after `base`'s — `base` (z = -5) has to be
        // ready and ordered before the shared node (`back`/`echo`, z 0/1)
        // gets composited on top of it.
        let mut distinct_nodes: Vec<&metal_graph::NodeId> = Vec::new();
        for id in &node_of_layer {
            if !distinct_nodes.contains(&id) {
                distinct_nodes.push(id);
            }
        }
        let node_pos = |id: &metal_graph::NodeId| {
            plan.ops
                .iter()
                .position(|op| &op.node == id)
                .expect("every node has an op")
        };
        let base_node = &node_of_layer[list.iter().position(|l| l.id == "base").unwrap()];
        let shared_node = &node_of_layer[list.iter().position(|l| l.id == "back").unwrap()];
        assert!(
            node_pos(base_node) < node_pos(shared_node),
            "the plan ordered the back layer's generator before the base layer's"
        );
    }

    // -----------------------------------------------------------------------
    // fan-out — the design notes PARTE IV §17.1/§17.4: a source can now feed more than
    // one destination, through `POST /layers` op "add" (unchanged from
    // outside) followed by the new op "route".
    // -----------------------------------------------------------------------

    /// The acceptance test this whole change exists to make pass: a single
    /// source, created on one device, routed onto a second one without
    /// touching the first — something that was structurally impossible
    /// before `Layer` moved out of `Device`'s own `Vec` and into a pool
    /// addressed by `Layer::outputs`. Exercises `apply_layers_op` directly
    /// (the same function `POST /layers` calls) rather than the HTTP layer,
    /// which this file has no fixture for — see that function's own note on
    /// why it was extracted.
    #[test]
    fn routing_a_layer_onto_a_second_device_makes_both_show_it() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());
        // Two real devices, not the built-in preview — the whole point is a
        // source feeding two actual destinations.
        state
            .outputs
            .list
            .push(Output::new("device-a", "a", 640, 360));
        state
            .outputs
            .list
            .push(Output::new("device-b", "b", 640, 360));
        state
            .sources
            .insert("device-a".into(), layers::Device::default());
        state
            .sources
            .insert("device-b".into(), layers::Device::default());
        let models = paint.model_count();
        let limits = state.limits;

        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "add", "device": "device-a", "model": 0,
            }),
        )
        .expect("add must succeed");
        assert_eq!(
            state.layers.len(),
            1,
            "add must create exactly one layer in the pool"
        );
        let lid = state.layers[0].id.clone();

        // Before routing: device-a shows it, device-b does not — a Layer
        // still cannot be in two places by accident.
        assert_eq!(state.layers_on("device-a").len(), 1);
        assert!(state.layers_on("device-b").is_empty());

        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "route", "layer": lid, "device": "device-b", "on": true,
            }),
        )
        .expect("route on:true must succeed");

        // This is exactly what was impossible before: the same Layer, drawn
        // on two devices at once — same id, same content.
        let on_a = state.layers_on("device-a");
        let on_b = state.layers_on("device-b");
        assert_eq!(on_a.len(), 1, "device-a must still show it");
        assert_eq!(on_b.len(), 1, "device-b must now show it too");
        assert_eq!(on_a[0].id, lid);
        assert_eq!(on_b[0].id, lid);
        assert_eq!(
            on_a[0], on_b[0],
            "the same layer, byte for byte, on both devices"
        );
        assert_eq!(
            state.layers.len(),
            1,
            "still one layer in the pool, not a copy"
        );

        // And routing off just one device leaves the other untouched — the
        // surgical half "remove"/"clear" are not.
        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "route", "layer": lid, "device": "device-a", "on": false,
            }),
        )
        .expect("route on:false must succeed");
        assert!(
            state.layers_on("device-a").is_empty(),
            "device-a must no longer show it"
        );
        assert_eq!(
            state.layers_on("device-b").len(),
            1,
            "device-b must be unaffected"
        );
    }

    /// V1: the layer editor's own edits through the route body
    /// (`apply_layers_op`, what `POST /layers` calls, rule 9). `set` writes
    /// blend + opacity; `raise` reorders the stack, and the device reads it
    /// back in `z` order — the reading the editor paints (rule 4).
    #[test]
    fn edit_layer_sets_blend_and_opacity_and_reorders_the_device_stack() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(Output::new("probe-editor", "editor", 640, 360));
        state
            .sources
            .insert("probe-editor".into(), layers::Device::default());
        let models = paint.model_count();
        let limits = state.limits;

        for name in ["fondo", "texto"] {
            apply_layers_op(
                &mut state,
                models,
                limits,
                &json!({ "op": "add", "device": "probe-editor", "model": 0, "name": name }),
            )
            .expect("add must succeed");
        }
        // The door's list is creation order; front-first is the `z` reading.
        let front_first = |state: &State| -> Vec<String> {
            let mut ls = state.routed_to("probe-editor");
            ls.sort_by(|a, b| b.z.cmp(&a.z));
            ls.iter().map(|l| l.name.clone()).collect()
        };
        assert_eq!(
            front_first(&state),
            vec!["texto".to_string(), "fondo".to_string()],
            "a freshly added layer lands in front"
        );

        let background = state
            .layers
            .iter()
            .find(|l| l.name == "fondo")
            .unwrap()
            .id
            .clone();

        // Blend + opacity through the same `set` op the editor posts.
        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({ "op": "set", "device": "probe-editor", "layer": background,
                     "blend": "multiply", "opacity": 0.4 }),
        )
        .expect("set must succeed");
        let layer = state.layers.iter().find(|l| l.id == background).unwrap();
        assert_eq!(layer.blend, Blend::Multiply, "blend reflected on the device");
        assert_eq!(layer.opacity, 0.4, "opacity reflected on the device");

        // Reorder through `raise`, the op the editor's up button posts.
        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({ "op": "raise", "device": "probe-editor", "layer": background }),
        )
        .expect("raise must succeed");
        assert_eq!(
            front_first(&state),
            vec!["fondo".to_string(), "texto".to_string()],
            "raise moves the layer in front"
        );
    }

    // The disk round trip for the same scenario — save a layer with two
    // entries in `outputs`, reload it, confirm both survive — lives in
    // `output::tests` as `a_fan_out_layer_survives_a_save_and_reload_through_outputs_json`,
    // next to `Outputs::save` and `load_devices_from`, which are private to
    // that module.

    // -----------------------------------------------------------------------
    // "duplicate" — the design notes PARTE IV §17.4's "duplicar rama": the one
    // genuinely new op this section asks for, on top of "route" above.
    // -----------------------------------------------------------------------

    /// **The acceptance test this whole op exists to make pass.** A source
    /// duplicated through `op:"duplicate"` must land on the very same
    /// `gen.*` node as its original in [`compile_device_plan`] — not a node
    /// that merely renders the same picture — because that shared node is
    /// what makes "twenty beans on rotations and distinct filters" cost one
    /// GPU render instead of twenty. [`GenKey`] already guarantees this for
    /// any two layers with the same `model`/`feed`/`base`/`library` (proven
    /// by `a_fan_out_stack_folds_two_layers_onto_one_generator`
    /// above); this test is that the duplicate op actually produces such a
    /// pair, by copying exactly those fields.
    #[test]
    fn duplicating_a_layer_shares_its_generator_with_the_original() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);
        let models = paint.model_count();
        let limits = state.limits;

        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "add", "device": PREVIEW, "model": 0,
            }),
        )
        .expect("add must succeed");
        assert_eq!(
            state.layers.len(),
            1,
            "add must create exactly one layer in the pool"
        );
        let original_id = state.layers[0].id.clone();

        let copy = apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "duplicate", "layer": original_id,
            }),
        )
        .expect("duplicate must succeed")
        .expect("duplicate must hand the new layer back in its Ok payload");
        assert_eq!(
            state.layers.len(),
            2,
            "duplicate must add exactly one layer to the pool"
        );
        assert_ne!(copy.id, original_id, "the copy must have its own id");
        assert_eq!(
            copy.outputs,
            vec![PREVIEW.to_string()],
            "no device named in the request — the copy inherits the original's own destination"
        );

        let air = state.airing(&mut sim, state.time());
        let manifest_ids = paint.ids();
        let (list, graph, plan, node_of_layer, _remote, _mode) = compile_device_plan(
            &state.preview,
            true,
            PREVIEW,
            &state.layers,
            &air,
            state.router.group(),
            0.0,
            &manifest_ids,
            models,
            state.overlays.on(PREVIEW, state.time()),
            &state.collections,
            // Nothing scheduled — none of these tests arm a `sequence::Run`.
            None,
            "laptop",
        )
        .expect("a non-empty stack must compile");

        let idx_of = |id: &str| list.iter().position(|l| l.id == id).unwrap();
        assert_eq!(
            node_of_layer[idx_of(&original_id)],
            node_of_layer[idx_of(&copy.id)],
            "a duplicated layer must share its generator node with the original"
        );

        // `Graph::compile` visits each node once regardless of fan-out
        // (§6 invariant 4) — so two layers on one shared node must compile
        // to one `out` op plus one generator op, not two generator ops.
        // This is the "free lunch of the topological walk" the design notes PARTE IV
        // §17.4 cites, exercised for real rather than assumed.
        let mut distinct_nodes: Vec<&metal_graph::NodeId> = Vec::new();
        for id in &node_of_layer {
            if !distinct_nodes.contains(&id) {
                distinct_nodes.push(id);
            }
        }
        assert_eq!(
            distinct_nodes.len(),
            1,
            "the original and its duplicate must resolve to exactly one generator node"
        );
        // The synthesized graph's `audio.live` (plan "Audio input is a
        // node") is a control source, not a drawing op: left out of the count.
        let drawing = plan
            .ops
            .iter()
            .filter(|op| {
                graph
                    .node(&op.node)
                    .is_some_and(|n| !n.kind.starts_with("audio."))
            })
            .count();
        assert_eq!(
            drawing, 2,
            "the compiled plan must have exactly one op for the shared generator plus one for \
             the output node — not one generator op per layer"
        );
    }

    /// The other half of the acceptance criteria: once duplicated, the copy
    /// is its own `Layer` — changing it must not reach back into the
    /// original, even though [`compile_device_plan`] may still fold the two
    /// onto one generator for rendering. `giro` (the design notes PARTE IV §17.2) is
    /// the field the request itself names as an example of what should vary
    /// "per branch".
    #[test]
    fn duplicating_a_layer_lets_the_copy_diverge_without_touching_the_original() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());
        let models = paint.model_count();
        let limits = state.limits;

        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "add", "device": PREVIEW, "model": 0,
            }),
        )
        .expect("add must succeed");
        let original_id = state.layers[0].id.clone();
        let original_giro_before = state.layers[0].turn;

        let copy = apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "duplicate", "layer": original_id,
            }),
        )
        .expect("duplicate must succeed")
        .expect("duplicate must hand the new layer back in its Ok payload");
        // Starts as an exact twin — "the same bean" — not a blank source.
        assert_eq!(copy.turn, original_giro_before);
        assert_eq!(copy.model, 0);

        let turned = 45.0_f64.to_radians();
        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "set", "layer": copy.id, "turn": turned,
            }),
        )
        .expect("set must succeed");

        let original_after = state.layers.iter().find(|l| l.id == original_id).unwrap();
        let copy_after = state.layers.iter().find(|l| l.id == copy.id).unwrap();
        assert_eq!(
            original_after.turn, original_giro_before,
            "turning the copy must not turn the original — they are independent instances"
        );
        assert!(
            (copy_after.turn - turned as f32).abs() < 1e-6,
            "the copy's own giro must have taken the new value"
        );
        assert_ne!(
            copy_after.turn, original_after.turn,
            "the two must actually disagree now"
        );
    }

    /// The request's own example: duplicate a source and send the copy to a
    /// *different* screen than the original, in one step, via the op's
    /// optional `device` field.
    #[test]
    fn duplicating_a_layer_onto_a_new_device_routes_only_the_copy_there() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(Output::new("device-a", "a", 640, 360));
        state
            .outputs
            .list
            .push(Output::new("device-b", "b", 640, 360));
        state
            .sources
            .insert("device-a".into(), layers::Device::default());
        state
            .sources
            .insert("device-b".into(), layers::Device::default());
        let models = paint.model_count();
        let limits = state.limits;

        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "add", "device": "device-a", "model": 0,
            }),
        )
        .expect("add must succeed");
        let original_id = state.layers[0].id.clone();

        let copy = apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "duplicate", "layer": original_id, "device": "device-b",
            }),
        )
        .expect("duplicate must succeed")
        .expect("duplicate must hand the new layer back in its Ok payload");

        assert_eq!(
            copy.outputs,
            vec!["device-b".to_string()],
            "a named device replaces the copy's outputs, it does not add to the original's"
        );
        assert_eq!(
            state.layers_on("device-a").len(),
            1,
            "device-a must still show only the original"
        );
        assert_eq!(
            state.layers_on("device-b").len(),
            1,
            "device-b must now show only the copy"
        );
        assert_eq!(state.layers_on("device-a")[0].id, original_id);
        assert_eq!(state.layers_on("device-b")[0].id, copy.id);

        let original_after = state.layers.iter().find(|l| l.id == original_id).unwrap();
        assert_eq!(
            original_after.outputs,
            vec!["device-a".to_string()],
            "the original's own routing must be untouched by a duplicate aimed elsewhere"
        );
    }

    // -----------------------------------------------------------------------
    // the design notes PARTE IV §17.5, first step: selecting a collection is reading
    // it, live, not copying it. The four tests below are this step's own
    // acceptance criteria — everything above this line about "apply" or
    // "mode: collection" predates the change; these are what prove it.
    // -----------------------------------------------------------------------

    /// **The acceptance test §17.5's first step exists for.** Select a
    /// collection from the mode dropdown, confirm the device draws its
    /// layer through the real render path, then edit the collection
    /// directly — no reselect, nothing sent to the device — and the next
    /// frame has to show the edit. Before this change, selecting copied the
    /// collection's layers into the pool once and the device never looked
    /// at `state.collections` again; this is what proves it now does.
    #[test]
    fn a_device_on_collection_mode_tracks_a_live_edit_to_the_collection() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());

        state.collections.list.push(Collection {
            id: "night-1".into(),
            name: "night".into(),
            layers: vec![Layer::new("l0", "entrance", 0)],
            composition: Composition::Overlap,
        });

        let models = paint.model_count();
        let limits = state.limits;
        let layers_before = state.layers.len();
        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "mode", "device": PREVIEW, "mode": "collection", "collection": "night-1",
            }),
        )
        .expect("selecting an existing collection must succeed");

        assert_eq!(
            state.stack(PREVIEW).unwrap().mode,
            layers::DeviceMode::Collection("night-1".to_string()),
            "the dropdown must record which collection is selected"
        );
        assert_eq!(
            state.layers.len(), layers_before,
            "selecting a collection must not copy its layers into the pool — the design notes PARTE IV §17.5"
        );

        let manifest_ids = paint.ids();
        let air = state.airing(&mut sim, state.time());
        let (list, _graph, _plan, _node_of_layer, _remote, _mode) = compile_device_plan(
            state.stack(PREVIEW).unwrap(),
            true,
            PREVIEW,
            &state.layers,
            &air,
            state.router.group(),
            0.0,
            &manifest_ids,
            models,
            state.overlays.on(PREVIEW, state.time()),
            &state.collections,
            // Nothing scheduled — none of these tests arm a `sequence::Run`.
            None,
            "laptop",
        )
        .expect("a device on a real collection must have something to draw");
        assert_eq!(
            list.len(),
            1,
            "the device must draw exactly the collection's own layer"
        );
        assert_eq!(list[0].turn, 0.0);

        // Edit the collection directly. No reselect, nothing sent to the device.
        state
            .collections
            .list
            .iter_mut()
            .find(|c| c.id == "night-1")
            .unwrap()
            .layers[0]
            .turn = 1.25;

        let air = state.airing(&mut sim, state.time());
        let (list, _graph, _plan, _node_of_layer, _remote, _mode) = compile_device_plan(
            state.stack(PREVIEW).unwrap(),
            true,
            PREVIEW,
            &state.layers,
            &air,
            state.router.group(),
            0.0,
            &manifest_ids,
            models,
            state.overlays.on(PREVIEW, state.time()),
            &state.collections,
            // Nothing scheduled — none of these tests arm a `sequence::Run`.
            None,
            "laptop",
        )
        .expect("still something to draw after editing the collection");
        assert_eq!(
            list[0].turn, 1.25,
            "a device on Collection(id) mode must read the collection live — an edit made \
             after selecting it must reach the next frame with no reselect"
        );
    }

    /// The other half of point 2: composition has to be read live off the
    /// collection too, not just the layers. Two layers sharing one generator
    /// (so both draw identical underlying pixels) with different z and
    /// opacity make Sectors and Overlap disagree on purpose — Sectors shows
    /// only the front layer, faded toward black by its own opacity
    /// (`a_faded_sector_goes_toward_black_not_toward_its_neighbour` in
    /// `compose.rs` is the same rule); Overlap blends it with the identical
    /// layer behind it and, since the two colours are equal, lands back on
    /// the unfaded one. Selecting the collection once and then flipping its
    /// saved composition, with no reselect, has to change which of those two
    /// the device renders.
    #[test]
    fn a_device_on_collection_mode_also_tracks_a_live_change_to_its_composition() {
        let mut paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());

        let mut back = Layer::new("back", "back", 0);
        back.z = 0;
        back.opacity = 1.0;
        back.blend = Blend::Normal;
        let mut front = Layer::new("front", "front", 0);
        front.z = 1;
        front.opacity = 0.3;
        front.blend = Blend::Normal;

        state.collections.list.push(Collection {
            id: "duo-1".into(),
            name: "duo".into(),
            layers: vec![back, front],
            composition: Composition::Sectors,
        });

        let models = paint.model_count();
        let limits = state.limits;
        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "mode", "device": PREVIEW, "mode": "collection", "collection": "duo-1",
            }),
        )
        .expect("selecting an existing collection must succeed");
        state.paused_at = Some(3.0);

        let (sectors_frame, _w, _h) =
            render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 16, 16);

        state
            .collections
            .list
            .iter_mut()
            .find(|c| c.id == "duo-1")
            .unwrap()
            .composition = Composition::Overlap;

        let (overlap_frame, _w, _h) =
            render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 16, 16);

        assert_ne!(
            sectors_frame, overlap_frame,
            "flipping the collection's saved composition, with no reselect, must change what \
             the device renders — composition has to be read live too, not just the layers"
        );
    }

    /// "apply" still copies onto the device — the design notes PARTE IV §17.5 only
    /// changes which mode the device is left in afterwards. Confirms both
    /// halves: the fork lands in `Own`, not `Collection(cid)` (see the
    /// "apply" op's own comment on why leaving it in `Collection` would make
    /// the fork it just made invisible, now that point 1 above is real), and
    /// the fork is genuinely independent of the collection it came from.
    #[test]
    fn applying_a_collection_forks_into_own_mode_independent_of_the_original() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());

        state.collections.list.push(Collection {
            id: "night-1".into(),
            name: "night".into(),
            layers: vec![Layer::new("l0", "entrance", 0)],
            composition: Composition::Overlap,
        });

        let models = paint.model_count();
        let limits = state.limits;
        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "apply", "device": PREVIEW, "collection": "night-1",
            }),
        )
        .expect("applying an existing collection must succeed");

        assert_eq!(
            state.stack(PREVIEW).unwrap().mode,
            layers::DeviceMode::Own,
            "apply must fork into an editable stack, not leave the device pointed at the \
             collection it forked from"
        );
        assert_eq!(
            state.layers.len(),
            1,
            "apply must still copy the collection's layer into the pool"
        );

        let copy_id = state.layers[0].id.clone();
        state.find_layer_mut(&copy_id).unwrap().turn = 2.0;

        let original = state.collections.find("night-1").unwrap();
        assert_eq!(
            original.layers[0].turn, 0.0,
            "editing the fork must not reach back into the saved collection it came from"
        );
    }

    /// The guard the design notes PARTE IV §17.5 asks for, carried over rather than
    /// lost when the copy came out of "mode: collection": reject a dropdown
    /// pointed at a collection that does not exist, at selection time, and
    /// touch nothing on the way to the error.
    #[test]
    fn selecting_a_missing_collection_is_rejected_and_mutates_nothing() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());
        let models = paint.model_count();
        let limits = state.limits;

        let mode_before = state.stack(PREVIEW).unwrap().mode.clone();
        let layers_before = state.layers.len();

        let err = apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "mode", "device": PREVIEW, "mode": "collection", "collection": "ghost",
            }),
        )
        .expect_err("a collection that does not exist must be rejected");
        assert_eq!(
            err, "no such collection",
            "the dropdown has to say what went wrong"
        );

        assert_eq!(
            state.stack(PREVIEW).unwrap().mode,
            mode_before,
            "a rejected selection must not change the device's mode"
        );
        assert_eq!(
            state.layers.len(),
            layers_before,
            "a rejected selection must not mutate the layer pool either"
        );
    }

    // -----------------------------------------------------------------------
    // Plan step 3 — the authored canvas driving a live device.
    //
    // The property every one of these is really about: attaching a graph must
    // never be able to make a device render something nobody can account for.
    // Either the graph covers the stack exactly and is used, or it does not
    // and synthesis runs unchanged. There is no third outcome, and the tests
    // below are the ones that would catch a third outcome appearing.
    // -----------------------------------------------------------------------

    /// A scratch data root, so none of this touches the repo's real `data/`.
    fn graph_scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-authored-{name}"));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(dir.join("graphs")).expect("scratch");
        dir
    }

    /// An authored graph for `device`: one `gen.<model>` per entry of
    /// `models`, each into `output.device`, in the order given.
    fn authored(device: &str, models: &[&str]) -> metal_graph::Graph {
        let mut g = metal_graph::Graph::new(
            device_graph_id(device),
            "authored",
            metal_graph::Domain::Live,
        );
        let out = g.add_node("out", "output.device", [8.0, 0.0]);
        for (i, m) in models.iter().enumerate() {
            let gen = g.add_node("gen", &format!("gen.{m}"), [0.0, i as f32 * 2.0]);
            g.add_edge(
                metal_graph::Endpoint::new(&gen, "image"),
                metal_graph::Endpoint::new(&out, "image"),
            );
        }
        g
    }

    #[test]
    fn ensure_device_graph_at_puts_the_screen_in_the_one_collection() {
        let root = graph_scratch("ensure-device-graph");
        ensure_device_graph_at(&root, "screen-1", "Screen 1");
        let g = load_graph_at(&root, "device-screen-1").expect("graph should exist");
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.nodes[0].kind, "output.device");
        let col = load_graph_at(&root, metal_graph::collection::CENTRAL_ID).expect("the central collection");
        assert_eq!(
            metal_graph::collection::bound_devices(&col).into_iter().collect::<Vec<_>>(),
            ["screen-1"],
            "the screen is a node of the one collection, not a collection of its own"
        );
        ensure_device_graph_at(&root, "screen-2", "Screen 2");
        let col = load_graph_at(&root, metal_graph::collection::CENTRAL_ID).unwrap();
        assert_eq!(
            col.nodes.iter().filter(|n| n.kind == "output.device").count(),
            2,
            "a second screen joins the same collection"
        );
    }

    #[test]
    fn ensure_device_graph_at_does_not_clobber_an_existing_graph() {
        let root = graph_scratch("ensure-device-graph-existing");
        let mut g = metal_graph::Graph::new(
            device_graph_id("screen-2"),
            "hand-authored",
            metal_graph::Domain::Live,
        );
        g.add_node("out", "output.device", [1.0, 2.0]);
        g.add_node("gen", "gen.spiral", [0.0, 0.0]);
        save_graph_at(&root, &g).unwrap();
        ensure_device_graph_at(&root, "screen-2", "Screen 2");
        let reloaded = load_graph_at(&root, "device-screen-2").unwrap();
        assert_eq!(
            reloaded.nodes.len(),
            2,
            "existing graph must survive untouched"
        );
    }

    /// The stack `authored_device_plan` is asked about, and the ingredients it
    /// needs to judge one. Uses the real `empty-1` collection so the layers are
    /// the same ones every other test in this file works with.
    fn stack_for_authored(
        paint: &dyn metal_perform::paint::Paint,
        state: &mut State,
        sim: &mut Listeners,
    ) -> (Vec<Layer>, layers::Airing, usize, Vec<&'static str>) {
        let c = crate::test_fixtures::collections()
            .find("empty-1")
            .expect("the empty-1 fixture")
            .clone();
        state.layers = stamped(c.layers, PREVIEW);
        state.preview.mode = layers::DeviceMode::Collection(c.id.clone());
        state.paused_at = Some(3.0);
        let air = state.airing(sim, state.time());
        let models = paint.model_count();
        let ids = paint.ids();
        let list: Vec<Layer> = layers::layers_on(&state.layers, PREVIEW)
            .into_iter()
            .cloned()
            .collect();
        (list, air, models, ids)
    }

    /// The safety property, stated directly: with nothing authored, the
    /// authored path declines and synthesis runs. Everything else in this file
    /// depends on this staying true, because every other test runs against a
    /// data root with no `device-*.json` in it.
    #[test]
    fn with_no_authored_graph_the_device_falls_back_to_synthesis() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let (list, air, models, ids) = stack_for_authored(paint.as_ref(), &mut state, &mut sim);
        let root = graph_scratch("absent");
        assert!(
            authored_device_plan_at(&root, PREVIEW, &list, &air, models, &ids).is_none(),
            "an empty graph directory must decline, not half-answer"
        );
    }

    /// The one that makes the feature real: a graph naming exactly the models
    /// this device is showing is used, and its nodes are the ones the renderer
    /// will be handed.
    #[test]
    fn an_authored_graph_that_matches_the_stack_is_the_one_that_renders() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let (list, air, models, ids) = stack_for_authored(paint.as_ref(), &mut state, &mut sim);
        let root = graph_scratch("match");

        let showing: Vec<&str> = list.iter().map(|l| ids[l.showing(&air, models)]).collect();
        save_graph_at(&root, &authored(PREVIEW, &showing)).expect("save");

        let (out_list, graph, _plan, node_of_layer) =
            authored_device_plan_at(&root, PREVIEW, &list, &air, models, &ids)
                .expect("a graph that names exactly what is showing must be used");
        assert_eq!(node_of_layer.len(), list.len(), "one node per layer");
        assert_eq!(
            out_list.len(),
            list.len(),
            "no config on any node — nothing folded away"
        );
        for (node, want) in node_of_layer.iter().zip(showing.iter()) {
            assert_eq!(graph.node(node).unwrap().kind, format!("gen.{want}"));
        }
    }

    /// A graph authored against a stack that has since changed. This is the
    /// case that would otherwise render the wrong model in silence — `showing`
    /// moves with automix and with the router, and neither asks the graph's
    /// permission.
    #[test]
    fn an_authored_graph_naming_the_wrong_model_falls_back_instead_of_drawing_it() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let (list, air, models, ids) = stack_for_authored(paint.as_ref(), &mut state, &mut sim);
        let root = graph_scratch("stale");

        // Right shape, wrong contents: as many generators as there are layers,
        // but one of them names a model this device is not showing.
        let mut showing: Vec<&str> = list.iter().map(|l| ids[l.showing(&air, models)]).collect();
        let other = ids
            .iter()
            .find(|id| **id != showing[0])
            .copied()
            .expect("more than one model");
        showing[0] = other;
        save_graph_at(&root, &authored(PREVIEW, &showing)).expect("save");

        assert!(
            authored_device_plan_at(&root, PREVIEW, &list, &air, models, &ids).is_none(),
            "a graph that disagrees with what is showing must decline, not draw its own idea"
        );
    }

    /// Partial coverage is refused whole — the all-or-nothing rule in
    /// `authored_device_plan`'s doc comment. A graph with fewer generators than
    /// there are layers would otherwise compose a frame that is half authored
    /// and half synthesized, which nobody could reason about.
    #[test]
    fn an_authored_graph_that_covers_only_some_layers_is_refused_whole() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let (list, air, models, ids) = stack_for_authored(paint.as_ref(), &mut state, &mut sim);
        if list.len() < 2 {
            return; // the fixture has only one layer; nothing partial to build
        }
        let root = graph_scratch("partial");
        let showing: Vec<&str> = list
            .iter()
            .take(list.len() - 1)
            .map(|l| ids[l.showing(&air, models)])
            .collect();
        save_graph_at(&root, &authored(PREVIEW, &showing)).expect("save");
        assert!(
            authored_device_plan_at(&root, PREVIEW, &list, &air, models, &ids).is_none(),
            "one generator short must fall back, not drop a layer"
        );
    }

    /// An `Offline` graph is not a device graph. `lint_domain_restricted_kinds`
    /// already forbids `output.device` outside `Live`, and this is the render
    /// path refusing to be the place that discovers it.
    #[test]
    fn an_offline_graph_is_not_accepted_as_a_device_graph() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let (list, air, models, ids) = stack_for_authored(paint.as_ref(), &mut state, &mut sim);
        let root = graph_scratch("offline");
        let showing: Vec<&str> = list.iter().map(|l| ids[l.showing(&air, models)]).collect();
        let mut g = authored(PREVIEW, &showing);
        g.domain = metal_graph::Domain::Offline;
        save_graph_at(&root, &g).expect("save");
        assert!(authored_device_plan_at(&root, PREVIEW, &list, &air, models, &ids).is_none());
    }

    /// B3 paso 4's own pixel-identity gate: a graph with generators but no
    /// `NodeLayerConfig` on any of them must hand back the layer list
    /// untouched — same values, same order — because that is the graph an
    /// operator who has never opened the new controls always has.
    #[test]
    fn a_graph_with_no_node_config_leaves_every_layer_field_untouched() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let (list, air, models, ids) = stack_for_authored(paint.as_ref(), &mut state, &mut sim);
        let root = graph_scratch("no-config");
        let showing: Vec<&str> = list.iter().map(|l| ids[l.showing(&air, models)]).collect();
        save_graph_at(&root, &authored(PREVIEW, &showing)).expect("save");

        let (out_list, _graph, _plan, _node_of_layer) =
            authored_device_plan_at(&root, PREVIEW, &list, &air, models, &ids)
                .expect("a graph that names exactly what is showing must be used");
        assert_eq!(
            out_list, list,
            "no config anywhere — the layer list must be byte for byte"
        );
    }

    /// The other half: a `gen.*` node's `NodeLayerConfig` actually reaches the
    /// layer it corresponds to, one field per test so a fold-two-fields bug
    /// cannot hide behind a fold-one-field pass.
    #[test]
    fn a_node_config_overrides_its_layer_blend_z_sector_mask_and_opacity() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        let (list, air, models, ids) = stack_for_authored(paint.as_ref(), &mut state, &mut sim);
        assert!(
            !list.is_empty(),
            "the empty-1 fixture must have at least one layer"
        );
        let root = graph_scratch("node-config");
        let showing: Vec<&str> = list.iter().map(|l| ids[l.showing(&air, models)]).collect();
        let mut g = authored(PREVIEW, &showing);

        let gens = authored_generators(&g);
        let cfg = node_layer::NodeLayerConfig {
            blend: Some(layers::Blend::Add),
            z: Some(9),
            sector: Some(layers::Sector {
                x: 0.1,
                y: 0.2,
                w: 0.3,
                h: 0.4,
            }),
            mask: Some("puerta".into()),
            opacity: Some(0.25),
            shader: None,
        };
        g.node_mut(&gens[0]).unwrap().config = cfg.to_config();
        save_graph_at(&root, &g).expect("save");

        let (out_list, _graph, _plan, _node_of_layer) =
            authored_device_plan_at(&root, PREVIEW, &list, &air, models, &ids)
                .expect("a graph that names exactly what is showing must be used");
        let before = &list[0];
        let after = &out_list[0];
        assert_eq!(after.blend, layers::Blend::Add);
        assert_eq!(after.z, 9);
        assert_eq!(
            after.sector,
            layers::Sector {
                x: 0.1,
                y: 0.2,
                w: 0.3,
                h: 0.4
            }
        );
        assert_eq!(after.mask.as_deref(), Some("puerta"));
        assert_eq!(after.opacity, 0.25);
        // Untouched fields prove this overrides, it does not rebuild the layer.
        assert_eq!(after.id, before.id);
        assert_eq!(after.model, before.model);
        if out_list.len() > 1 {
            assert_eq!(
                out_list[1], list[1],
                "only the configured node's layer changed"
            );
        }
    }

    /// `authored_generators` has to see *through* an effect chain to the
    /// generator behind it — otherwise attaching a single `fx.*` node would
    /// make the whole graph look like it names no models at all, and every
    /// authored device would silently fall back the moment it got interesting.
    #[test]
    fn a_generator_behind_a_chain_of_effects_is_still_found() {
        let mut g = metal_graph::Graph::new("device-x", "x", metal_graph::Domain::Live);
        let out = g.add_node("out", "output.device", [8.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let a = g.add_node("fx", "fx.zoom", [2.0, 0.0]);
        let b = g.add_node("fx", "fx.vignette", [4.0, 0.0]);
        g.add_edge(
            metal_graph::Endpoint::new(&gen, "image"),
            metal_graph::Endpoint::new(&a, "image"),
        );
        g.add_edge(
            metal_graph::Endpoint::new(&a, "image"),
            metal_graph::Endpoint::new(&b, "image"),
        );
        g.add_edge(
            metal_graph::Endpoint::new(&b, "image"),
            metal_graph::Endpoint::new(&out, "image"),
        );
        assert_eq!(
            authored_generators(&g),
            vec![gen],
            "two effects deep, still the generator"
        );
    }

    /// A graph with no output node names nothing, rather than panicking or
    /// answering with whatever happened to be first.
    #[test]
    fn a_graph_with_no_output_node_names_no_generators() {
        let mut g = metal_graph::Graph::new("device-y", "y", metal_graph::Domain::Live);
        g.add_node("gen", "gen.turing", [0.0, 0.0]);
        assert!(authored_generators(&g).is_empty());
    }

    /// One generator feeding the output twice is how an authored graph spells
    /// the fan-out `GenKey` produces when synthesizing. It must read back as
    /// two layers on one node, not as one layer.
    #[test]
    fn one_generator_wired_to_the_output_twice_stands_for_two_layers() {
        let mut g = metal_graph::Graph::new("device-z", "z", metal_graph::Domain::Live);
        let out = g.add_node("out", "output.device", [8.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.add_edge(
            metal_graph::Endpoint::new(&gen, "image"),
            metal_graph::Endpoint::new(&out, "image"),
        );
        g.add_edge(
            metal_graph::Endpoint::new(&gen, "image"),
            metal_graph::Endpoint::new(&out, "image"),
        );
        assert_eq!(authored_generators(&g), vec![gen.clone(), gen]);
    }

    // -----------------------------------------------------------------------
    // The `/outputs add` → `/layers add` gap.
    //
    // Not part of any plan — found because it aborts `check.mjs` partway and
    // therefore blocked live verification of everything else. Kept here rather
    // than only in `check.mjs` because a socket test cannot say *why* it broke
    // and this can.
    // -----------------------------------------------------------------------

    /// A device created while the app is running must be usable immediately,
    /// not after a restart.
    ///
    /// `State::new` backfills `sources` for every stored output, so this
    /// always worked across a restart and never worked within a session —
    /// which is exactly the shape of bug that survives a test suite made of
    /// freshly-constructed `State`s.
    #[test]
    fn a_device_added_at_runtime_can_take_a_layer_without_a_restart() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());
        let models = paint.model_count();
        let limits = state.limits;

        assert!(
            state.stack("wall").is_none(),
            "not there before it is added"
        );

        // What `POST /outputs {op:"add"}` does, by the same two writes.
        state
            .outputs
            .list
            .push(Output::new("wall", "Wall", 1920, 1080));
        state
            .sources
            .entry("wall".to_string())
            .or_insert_with(layers::Device::default);

        assert!(
            state.stack("wall").is_some(),
            "a device must exist the instant its output does — otherwise every \
             /layers op on it answers 'no such device' about a device that is \
             sitting in the list right beside it"
        );

        apply_layers_op(
            &mut state,
            models,
            limits,
            &json!({
                "op": "add", "device": "wall", "model": 0,
            }),
        )
        .expect("a freshly created device must accept a layer");
        assert_eq!(layers::layers_on(&state.layers, "wall").len(), 1);
    }

    /// Removing a device must take its composition and mode with it, or the
    /// next device to reuse that id inherits settings nobody chose.
    #[test]
    fn removing_a_device_forgets_its_composition_and_mode() {
        let paint = paint::painter();
        let mut state = State::new(paint.as_ref());

        state
            .outputs
            .list
            .push(Output::new("wall", "Wall", 1920, 1080));
        state
            .sources
            .entry("wall".to_string())
            .or_insert_with(layers::Device::default);
        // `Disabled`, not `Automix`: `DeviceMode::default()` *is* `Automix`
        // (`layers.rs`), so setting that and then re-adding would read as
        // "reset" whether the entry was really dropped or not. A test that
        // cannot fail is not a test.
        state.stack_mut("wall").unwrap().mode = layers::DeviceMode::Disabled;

        state.outputs.list.retain(|o| o.id != "wall");
        state.sources.remove("wall");
        assert!(state.stack("wall").is_none());

        state
            .outputs
            .list
            .push(Output::new("wall", "Wall", 1920, 1080));
        state
            .sources
            .entry("wall".to_string())
            .or_insert_with(layers::Device::default);
        assert!(
            !matches!(
                state.stack("wall").unwrap().mode,
                layers::DeviceMode::Disabled
            ),
            "a re-added device must start fresh, not resume the deleted one's mode — \
             a screen you turned off, deleted and put back must come back on"
        );
    }

    // -----------------------------------------------------------------------
    // S.R mitad A point 4 — a device compiles only on the machine that
    // composes it; otherwise it is not compiled here and renders black.
    // -----------------------------------------------------------------------

    /// A screen this machine only shows — someone else composes it — must
    /// not be compiled here even though its stack has real layers: compiling
    /// it locally would be exactly the fallback S.R rules out.
    #[test]
    fn a_screen_composed_elsewhere_is_not_compiled_here() {
        let mut wall = Output::new("wall", "Wall", 100, 100);
        wall.composed_on = Some("pi".to_string());
        let outputs = Outputs { list: vec![wall] };
        assert!(!composes_here(&outputs, "wall", "laptop"));

        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.layers = fan_out_stack();
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);

        let air = state.airing(&mut sim, state.time());
        let manifest_ids = paint.ids();
        let result = compile_device_plan(
            &state.preview,
            false,
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
        );
        assert!(
            result.is_none(),
            "composed_here=false must black out a stack that otherwise has layers"
        );
    }

    /// Shown elsewhere but composed here: `composed_on` is the one that
    /// decides, not which machine the screen is plugged into.
    #[test]
    fn a_screen_shown_elsewhere_but_composed_here_is_compiled_here() {
        let mut wall = Output::new("wall", "Wall", 100, 100);
        wall.node = Some("pi".to_string());
        wall.composed_on = Some("laptop".to_string());
        let outputs = Outputs { list: vec![wall] };
        assert!(composes_here(&outputs, "wall", "laptop"));
    }

    /// The preview has no `Output` entry at all — nothing to defer to, so it
    /// always composes on whichever machine is running it.
    #[test]
    fn a_screen_with_no_output_entry_is_the_preview_and_composes_here() {
        assert!(composes_here(&Outputs::default(), PREVIEW, "laptop"));
    }

    // -----------------------------------------------------------------------
    // `Output::ceded_to` — a screen handed off to another program on the
    // same machine renders black locally, the same posture as one nobody
    // composes here.
    // -----------------------------------------------------------------------

    /// An `Output` with `ceded_to` set reports ceded.
    #[test]
    fn a_screen_ceded_to_another_program_is_ceded() {
        let mut wall = Output::new("wall", "Wall", 100, 100);
        wall.ceded_to = Some("resolume".to_string());
        let outputs = Outputs { list: vec![wall] };
        assert!(is_ceded(&outputs, "wall"));
    }

    /// An `Output` with no `ceded_to` is not ceded.
    #[test]
    fn a_screen_with_no_ceded_to_is_not_ceded() {
        let wall = Output::new("wall", "Wall", 100, 100);
        let outputs = Outputs { list: vec![wall] };
        assert!(!is_ceded(&outputs, "wall"));
    }

    /// The preview has no `Output` entry at all, so it is never ceded.
    #[test]
    fn a_screen_with_no_output_entry_is_the_preview_and_is_never_ceded() {
        assert!(!is_ceded(&Outputs::default(), PREVIEW));
    }

    /// `render_device`'s own guard mirrors `compile_device_plan`'s: a device
    /// ceded to another program renders `black(w, h)` even though its stack
    /// has real layers — the same shape as
    /// `a_screen_composed_elsewhere_is_not_compiled_here`, but through the
    /// full render function rather than the pure predicate.
    #[test]
    fn a_ceded_screen_renders_black() {
        let mut paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());

        let mut wall = Output::new("wall", "Wall", 100, 100);
        wall.ceded_to = Some("resolume".to_string());
        state.outputs.list.push(wall);
        state
            .sources
            .entry("wall".to_string())
            .or_insert_with(layers::Device::default);
        state.layers = fan_out_stack();
        if let Some(stack) = state.stack_mut("wall") {
            stack.mode = layers::DeviceMode::Own;
        }
        state.paused_at = Some(3.0);

        let (pixels, w, h) = render_device(&mut *paint, &mut sim, &mut state, "wall", 100, 100);
        assert_eq!((w, h), (100, 100));
        assert_eq!(pixels, black(w, h), "a ceded device must render black");
    }

    /// Four screens, each shown by its own machine and composed nowhere in
    /// particular — every machine composes exactly the one it shows.
    #[test]
    fn four_laptops_each_compose_and_render_their_own() {
        let outputs = Outputs {
            list: (1..=4)
                .map(|i| {
                    let mut o = Output::new(&format!("d{i}"), &format!("D{i}"), 100, 100);
                    o.node = Some(format!("n{i}"));
                    o
                })
                .collect(),
        };
        for local in ["n1", "n2", "n3", "n4"] {
            let composes_count = outputs
                .list
                .iter()
                .filter(|o| composes_here(&outputs, &o.id, local))
                .count();
            let owns_count = outputs
                .list
                .iter()
                .filter(|o| o.node.as_deref() == Some(local))
                .count();
            assert_eq!(
                composes_count, owns_count,
                "a machine must compose exactly the screens it shows and no others"
            );
            assert_eq!(
                composes_count, 1,
                "each laptop shows exactly one of the four"
            );
        }
    }

    /// The same four screens, all composed on one laptop instead: that
    /// machine composes every one of them, and none of the others compose any.
    #[test]
    fn one_laptop_composes_for_four_screens() {
        let outputs = Outputs {
            list: (1..=4)
                .map(|i| {
                    let mut o = Output::new(&format!("d{i}"), &format!("D{i}"), 100, 100);
                    o.node = Some(format!("n{i}"));
                    o.composed_on = Some("laptop".to_string());
                    o
                })
                .collect(),
        };
        let composes_on_laptop = outputs
            .list
            .iter()
            .filter(|o| composes_here(&outputs, &o.id, "laptop"))
            .count();
        assert_eq!(
            composes_on_laptop,
            outputs.list.len(),
            "one laptop composing for every screen must compose all of them"
        );

        let composes_on_n1 = outputs
            .list
            .iter()
            .filter(|o| composes_here(&outputs, &o.id, "n1"))
            .count();
        assert_eq!(
            composes_on_n1, 0,
            "the machine that only shows a screen does not compose it"
        );
    }

    /// An empty default would silently make every `node`/`composed_on`
    /// comparison against `local` match nothing instead of the real machine.
    #[test]
    fn the_hostname_default_is_never_empty() {
        assert!(!local_node_default().is_empty());
    }

    // -----------------------------------------------------------------------
    // `Layer::render_on` — a layer pinned to a machine is excluded from a
    // plan compiled anywhere else, and reported back rather than compiled
    // here. The transport that would actually ship its frame there does not
    // exist yet (S.R punto 5) — these tests only cover the data-model half.
    // -----------------------------------------------------------------------

    /// A layer explicitly pinned to the machine that is, in fact, compiling
    /// it must produce exactly what leaving it unpinned would — pinning to
    /// "here" is not a distinct behaviour, it is `Each` spelled out.
    #[test]
    fn render_on_never_picks_a_model() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();

        let mut pinned_state = State::new(paint.as_ref());
        let mut layer = Layer::new("solo", "solo", 0);
        layer.render_on = layers::RenderOn::Node("laptop".to_string());
        pinned_state.layers = stamped(vec![layer], PREVIEW);
        pinned_state.preview.mode = layers::DeviceMode::Own;
        pinned_state.paused_at = Some(3.0);

        let mut each_state = State::new(paint.as_ref());
        let mut layer = Layer::new("solo", "solo", 0);
        layer.render_on = layers::RenderOn::Each;
        each_state.layers = stamped(vec![layer], PREVIEW);
        each_state.preview.mode = layers::DeviceMode::Own;
        each_state.paused_at = Some(3.0);

        let manifest_ids = paint.ids();
        let air = pinned_state.airing(&mut sim, pinned_state.time());
        let (_list, pinned_graph, _plan, _node_of_layer, pinned_remote, _mode) =
            compile_device_plan(
                &pinned_state.preview,
                true,
                PREVIEW,
                &pinned_state.layers,
                &air,
                pinned_state.router.group(),
                0.0,
                &manifest_ids,
                paint.model_count(),
                pinned_state.overlays.on(PREVIEW, pinned_state.time()),
                &pinned_state.collections,
                None,
                "laptop",
            )
            .expect("a layer pinned to the compiling machine still compiles");

        let air = each_state.airing(&mut sim, each_state.time());
        let (_list, each_graph, _plan, _node_of_layer, each_remote, _mode) = compile_device_plan(
            &each_state.preview,
            true,
            PREVIEW,
            &each_state.layers,
            &air,
            each_state.router.group(),
            0.0,
            &manifest_ids,
            paint.model_count(),
            each_state.overlays.on(PREVIEW, each_state.time()),
            &each_state.collections,
            None,
            "laptop",
        )
        .expect("the same stack left unpinned still compiles");

        assert_eq!(
            pinned_graph.to_json(),
            each_graph.to_json(),
            "Node(local) must compile to exactly the same graph as Each"
        );
        assert!(
            pinned_remote.is_empty(),
            "a layer pinned to the local machine is not remote"
        );
        assert!(each_remote.is_empty());
    }

    /// A layer pinned to some other machine leaves the local plan and comes
    /// back in `remote` instead — the layer left `Each` stays, unaffected.
    #[test]
    fn a_layer_rendered_elsewhere_leaves_the_plan_and_enters_remote() {
        let paint = paint::painter();
        let mut sim = Listeners::stand_in();
        let mut state = State::new(paint.as_ref());

        let mut here = Layer::new("here", "here", 0);
        here.render_on = layers::RenderOn::Each;
        let mut elsewhere = Layer::new("elsewhere", "elsewhere", 0);
        elsewhere.render_on = layers::RenderOn::Node("pi".to_string());
        state.layers = stamped(vec![here, elsewhere], PREVIEW);
        state.preview.mode = layers::DeviceMode::Own;
        state.paused_at = Some(3.0);

        let air = state.airing(&mut sim, state.time());
        let manifest_ids = paint.ids();
        let (list, _graph, _plan, _node_of_layer, remote, _mode) = compile_device_plan(
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
        .expect("the Each layer alone still leaves something to draw");

        let ids: Vec<&str> = list.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["here"], "only the Each layer stays in the local plan");
        let pinned: Vec<(&str, &str, usize)> = remote
            .iter()
            .map(|r| (r.layer.id.as_str(), r.node.as_str(), r.pos))
            .collect();
        assert_eq!(
            pinned,
            vec![("elsewhere", "pi", 1)],
            "the pinned layer is reported, not compiled, here, and keeps its place (after `here`)"
        );
    }
}

/// Android: copies the APK's `assets/data` into `<files>/data` when the build
/// stamp (`data/build-stamp`, written by build-apk.sh) differs from the staged one.
/// Returns the staged `data` directory.
#[cfg(target_os = "android")]
fn stage_android_data(
    app: &winit::platform::android::activity::AndroidApp,
    files: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    use std::ffi::CString;
    use std::io::Read;
    let am = app.asset_manager();
    let data = files.join("data");
    let stamp = {
        let mut asset = CString::new("data/build-stamp")
            .ok()
            .and_then(|c| am.open(&c))
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "data/build-stamp"))?;
        let mut buf = Vec::new();
        asset.read_to_end(&mut buf)?;
        buf
    };
    if metal_vfs::read(data.join("build-stamp")).ok().as_deref() == Some(stamp.as_slice()) {
        return Ok(data);
    }
    fn copy_dir(
        am: &winit::platform::android::activity::ndk::asset::AssetManager,
        rel: &str,
        dst: &std::path::Path,
    ) -> std::io::Result<()> {
        use std::ffi::CString;
        use std::io::Read;
        metal_vfs::create_dir_all(dst)?;
        let Ok(c) = CString::new(rel) else {
            return Ok(());
        };
        let Some(dir) = am.open_dir(&c) else {
            return Ok(());
        };
        for name in dir {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let child = format!("{rel}/{name}");
            let target = dst.join(&name);
            match CString::new(child.as_str()).ok().and_then(|c| am.open(&c)) {
                Some(mut asset) => {
                    let mut buf = Vec::new();
                    asset.read_to_end(&mut buf)?;
                    metal_vfs::write(&target, buf)?;
                }
                None => copy_dir(am, &child, &target)?,
            }
        }
        Ok(())
    }
    copy_dir(&am, "data", &data)?;
    metal_vfs::write(data.join("build-stamp"), &stamp)?;
    Ok(data)
}

/// Android: `android-activity` calls this from the native activity. The
/// `AndroidApp` goes to `shell::window` (the event loop needs it) and the
/// app's internal storage becomes the data root, since the process cwd is not
/// a writable place on a phone. Everything else is the desktop windowed path.
/// Android's logcat is what `adb logcat` shows, and the process's stdout and
/// stderr go nowhere by default, so every `eprintln!` (the slow-frame and
/// perf lines, the adapter's name, the staging errors) would be lost. Points
/// both at a pipe read by a thread that writes each line to logcat under
/// the tag `metal-dash`. Best effort: a failed `pipe` leaves the streams as they
/// were.
#[cfg(target_os = "android")]
fn route_stdio_to_logcat() {
    use std::ffi::{c_char, c_int, CString};
    use std::io::{BufRead, BufReader};
    use std::os::fd::FromRawFd;
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
    unsafe extern "C" {
        fn pipe(fds: *mut c_int) -> c_int;
        fn dup2(old: c_int, new: c_int) -> c_int;
        fn close(fd: c_int) -> c_int;
    }
    const ANDROID_LOG_INFO: c_int = 4;
    let mut fds = [0 as c_int; 2];
    // SAFETY: plain POSIX calls on fds this process owns; the reader takes
    // ownership of the read end and nothing else holds it.
    unsafe {
        if pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        dup2(fds[1], 1);
        dup2(fds[1], 2);
        close(fds[1]);
    }
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let _ = std::thread::Builder::new()
        .name("logcat".into())
        .spawn(move || {
            let tag = c"metal-dash";
            for line in BufReader::new(read).lines().map_while(Result::ok) {
                if let Ok(text) = CString::new(line) {
                    // SAFETY: both strings are NUL-terminated and live for the call.
                    unsafe { __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), text.as_ptr()) };
                }
            }
        });
}

/// The sync node (`metal-sync`) in-process, on its own thread. Its data root is
/// the app's `data/` (the same tree the shell reads), its keys and iroh stores
/// live under `<files>/metal-sync`, and its control surface is `127.0.0.1:` `FW_SYNC_PORT` (7901 by default),
/// where `GET /frame`, pairing and the song pull answer as on the desktop.
#[cfg(target_os = "android")]
fn start_sync_node(files: &std::path::Path) {
    let dir = files.join("metal-sync");
    let opts = metal_sync::node::NodeOpts {
        paths: metal_sync::node::NodePaths {
            data: files.join("data"),
            key: dir.join("node.key"),
            sync: dir.join("sync.toml"),
            config_dir: dir.clone(),
            store: dir.join("store"),
        },
        ticket: None,
        mode: metal_sync::node::Mode::Run,
        control_host: "127.0.0.1".to_string(),
        control_port: call::author::playout::sync_port(),
        dash_port: 7900,
    };
    if let Err(e) = metal_sync::node::spawn_thread(opts) {
        eprintln!("  metal-dash: cannot start metal-sync: {e}");
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    route_stdio_to_logcat();
    if let Some(files) = app.internal_data_path() {
        let _ = metal_vfs::create_dir_all(&files);
        // Android has no `$HOME`, and the Library walks `$HOME` by default.
        // The app's own files dir is where a pushed clip lands and what the
        // cwd is, so that is the home the library and `/fs` start from.
        std::env::set_var("HOME", &files);
        start_sync_node(&files);
        // The read-only data ships as APK assets (`data/`, packed by
        // scripts/build-apk.sh). Staged once per build into `<files>/data`,
        // then the cwd moves to `<files>`: the literal `data/...` paths the
        // author code uses resolve there, and `data_root()` is that same dir.
        match stage_android_data(&app, &files) {
            Ok(data) => {
                let _ = std::env::set_current_dir(&files);
                output::set_data_root(&data);
            }
            Err(e) => eprintln!("  metal-dash: cannot stage android data: {e}"),
        }
    }
    shell::window::set_android_app(app);
    let cli = <Cli as clap::Parser>::parse_from(["metal-dash"]);
    migrate_collections_to_graphs_once();
    if let Err(e) = main_windowed(cli) {
        eprintln!("  metal-dash: {e}");
    }
}
