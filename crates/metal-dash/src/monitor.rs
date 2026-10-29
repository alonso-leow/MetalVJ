//! `S.U16`: the monitor — the preview `O.11` paso 3 externalised. A
//! `winit` window, fullscreen on whichever monitor `choose` picks (an
//! explicit `--display` connector name overrides it; otherwise the monitor
//! named like the device wins, else the first external one over a laptop
//! panel) whose only job is painting what `GET /frame` returns: it
//! **does not render** (the slaving rule, `O.8` — *slave to the
//! pixels* — stays untouched: no painter, no registry, no egui in here);
//! it decodes the returned JPEG and prints it through `softbuffer`.
//!
//! **The transport.** Frames ask the local `metal-sync` first:
//! `GET :7901/frame?device=D` (7901 = `FW_SYNC_PORT`, the default) (the design notes, "Frames that travel between
//! machines"). `metal-sync` resolves D's `composed_on`/`node` through its
//! `name ↔ node_id` table and either loops back to this machine's metal-dash
//! or fetches the frame from the peer over QUIC (`fw/frame/1`). Whatever it
//! answers — a JPEG, a 404 for a name nobody carries, a 409 for a ceded
//! device — is final. Only when nothing listens on `:7901` at all (a
//! machine with no `metal-sync`) does the mode ask its own metal-dash (`--port`)
//! directly, which is the same local frame `metal-sync` would have looped back
//! to. The invocation is still a CLI (`--monitor [device]`); being enthroned
//! by `Output.composed_on` rather than a flag is not built yet.
//!
//! **Rate and failure are the plan's numbers.** One blocking `GET` per
//! redraw, paced by winit's own `about_to_wait` → `request_redraw` (never
//! a timer of this module's). A failed fetch (no master up, device gone
//! from `data/outputs.json`) paints black and keeps asking — no fallback
//! to compose or render locally, per `S.R`'s *"no fallback"*.
//!
//! The event loop owns the thread; the fetch is a plain blocking TCP GET
//! on the same thread — this is what "no async runtime" looks like when
//! the redraw request itself is the pacer.

use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::num::NonZeroU32;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

/// `main`'s `--monitor` entry: the loop runs on the calling thread, so a
/// stalled monitor stalls only itself, never the HTTP server of some
/// master instance.
pub fn run(port: u16, device: String, display: Option<String>) {
    let event_loop = EventLoop::new().expect("event loop init failed");
    let mut monitor = Monitor::new(port, device, display);
    event_loop
        .run_app(&mut monitor)
        .expect("event loop exited with an error");
}

/// Everything the mode owns. `resolved` backs the once-per-run query the
/// bare `--monitor` invocation needs to learn a device name from the
/// master's own `/monitor` route — an explicit `--monitor <device>` skips
/// it entirely. `display` is the operator's `--display` override for which
/// monitor `choose` picks.
struct Monitor {
    port: u16,
    device: String,
    display: Option<String>,
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    resolved: bool,
    /// Set the moment `GET /frame` answers 409: the device has been ceded
    /// to something else (VJ software, say), so this window has let go of
    /// the screen — not fullscreen, minimized — until the device comes
    /// back.
    ceded: bool,
    /// The monitor `resumed()` picked, kept so `about_to_wait` can hand the
    /// window back to the *same* monitor on recovery without re-running the
    /// picking logic or recreating the window.
    chosen_monitor: Option<MonitorHandle>,
    /// Last time the ceded-recovery poll actually ran, so `about_to_wait`
    /// can throttle it to roughly 2 Hz instead of hammering the master on
    /// every event-loop iteration.
    last_ceded_poll: Option<web_time::Instant>,
}

impl Monitor {
    fn new(port: u16, device: String, display: Option<String>) -> Monitor {
        Monitor {
            port,
            device,
            display,
            window: None,
            context: None,
            surface: None,
            resolved: false,
            ceded: false,
            chosen_monitor: None,
            last_ceded_poll: None,
        }
    }

    /// The one job: resolve the device once, fetch, decode, scale, print.
    /// Failure anywhere paints black and keeps the loop going — the honest
    /// answer while the master is down, and the only behaviour the
    /// slaving rule permits (no local compose/render, ever).
    fn redraw(&mut self) {
        let Some(window) = &self.window else { return };
        let Some(surface) = &mut self.surface else {
            return;
        };

        if self.ceded {
            // `about_to_wait` owns the recovery poll (and its throttle)
            // while minimized — a stray `RedrawRequested` here must not
            // race it by fetching and deciding independently.
            return;
        }

        if !self.resolved {
            self.resolved = true;
            if self.device.is_empty() {
                // A bare `--monitor` asks the master's own `/monitor`
                // route which device is on the air — the same read
                // `out.html`'s select box populates itself from.
                self.device = first_source_device(self.port).unwrap_or_default();
            }
        }

        let (width, height): (u32, u32) = window.inner_size().into();
        let (Some(nz_width), Some(nz_height)) = (NonZeroU32::new(width), NonZeroU32::new(height))
        else {
            return;
        };
        // softbuffer refuses `buffer_mut` until the surface has been told
        // its size, and the size follows the fullscreen monitor, not us.
        if surface.resize(nz_width, nz_height).is_err() {
            return;
        }

        let frame = match fetch_jpeg(self.port, &self.device) {
            Ok(frame) => Some(frame),
            // The device has been ceded to something else: let go of the
            // screen (the one shared transition spot — `about_to_wait`
            // takes it from here) and paint nothing this frame.
            Err(FrameError::Ceded) => {
                enter_ceded(window, &mut self.ceded);
                return;
            }
            // No master, no device, bad wire, bad jpeg: black this frame,
            // keep asking the next — `S.R`'s *"no fallback to compose or
            // render locally"*.
            Err(FrameError::Unavailable) => None,
        };

        let (frame_w, frame_h, rgb) = match frame {
            Some((w, h, rgb)) => (w, h, rgb),
            None => (0, 0, Vec::new()),
        };
        let mut frame_buffer = surface.buffer_mut().expect("buffer_mut");
        frame_buffer.fill(0xFF00_0000);
        if frame_w > 0 && frame_h > 0 && rgb.len() >= (frame_w * frame_h * 3) as usize {
            let buffer_slice: &mut [u32] = &mut frame_buffer;
            paint_scaled(frame_w, frame_h, &rgb, width, height, buffer_slice);
        }
        frame_buffer.present().expect("present frame");
    }
}

impl ApplicationHandler for Monitor {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let handles: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        let infos: Vec<MonitorInfo> = handles.iter().map(MonitorInfo::from_handle).collect();

        if let Some(display) = &self.display {
            if !matches_display(&infos, display) {
                let names: Vec<&str> = infos
                    .iter()
                    .map(|m| m.name.as_deref().unwrap_or("?"))
                    .collect();
                eprintln!(
                    "  monitor: no display named \"{display}\" — available: {}",
                    names.join(", ")
                );
            }
        }

        let chosen = choose(&infos, self.display.as_deref(), &self.device);
        let handle = chosen.and_then(|i| handles.get(i)).cloned();
        // Kept so a later 409-recovery can hand the window back to this
        // same monitor without re-running the picking logic.
        self.chosen_monitor = handle.clone();
        if let Some(info) = chosen.and_then(|i| infos.get(i)) {
            eprintln!(
                "  monitor: {} {}x{} at ({},{}) — device {}",
                info.name.as_deref().unwrap_or("?"),
                info.size.0,
                info.size.1,
                info.position.0,
                info.position.1,
                self.device,
            );
        }

        let attributes = Window::default_attributes()
            .with_title("MetalVJ monitor")
            .with_fullscreen(Some(Fullscreen::Borderless(handle)));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("create_window failed"),
        );
        window.set_cursor_visible(false);
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        window.request_redraw();
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // The surface (and the context under it) holds a window reference
        // that must be released before the next `resumed` re-creates them.
        self.surface = None;
        self.context = None;
        self.window = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.logical_key == Key::Named(NamedKey::Escape) && event.state.is_pressed() {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(window) = &self.window else { return };

        if !self.ceded {
            // Unthrottled, exactly as before ceding was a concept.
            window.request_redraw();
            return;
        }

        // Minimized windows can stop getting `RedrawRequested` at all on
        // some platforms (niri/Wayland included) once unmapped, so the
        // recovery poll is driven from here — every event-loop iteration,
        // regardless of window visibility — rather than from the redraw
        // cycle, throttled to roughly 2 Hz so it doesn't hammer the master.
        let now = web_time::Instant::now();
        if !poll_due(self.last_ceded_poll, now) {
            return;
        }
        self.last_ceded_poll = Some(now);

        if fetch_jpeg(self.port, &self.device).is_ok() {
            self.ceded = false;
            window.set_minimized(false);
            window.set_fullscreen(Some(Fullscreen::Borderless(self.chosen_monitor.clone())));
            // Paint the recovered frame — `redraw()` will fetch it again,
            // which is the only fetch that touches the surface buffer.
            window.request_redraw();
        }
    }
}

/// The one shared spot for the "device just got ceded" transition, so
/// `redraw()`'s own fetch is the only place it happens — never duplicated
/// against `about_to_wait`'s recovery check.
fn enter_ceded(window: &Window, ceded: &mut bool) {
    *ceded = true;
    window.set_fullscreen(None);
    window.set_minimized(true);
}

/// Why a frame fetch didn't produce pixels — the only distinction the
/// window state machine cares about.
enum FrameError {
    /// `GET /frame` answered 409 — the device is ceded to something else.
    Ceded,
    /// Anything else: no master, no device, a bad wire, a bad JPEG — the
    /// existing "paint black, keep asking, no fallback" behaviour.
    Unavailable,
}

/// One blocking HTTP/1.1 GET of the frame route, loopback only — plain
/// `http`, no TLS: `metal-sync` on its control port (`FW_SYNC_PORT`, 7901 by
/// default) when it is listening, this
/// machine's metal-dash otherwise (see the module doc). The
/// shape is `metal-agent::http`'s own outbound parser: status line, headers,
/// Content-Length or read-to-EOF (this client always sends
/// `Connection: close`).
fn fetch_jpeg(port: u16, device: &str) -> Result<(u32, u32, Vec<u8>), FrameError> {
    // `fmt=qoi`: a loopback ask, so the master skips the JPEG encode; a master
    // (or `metal-sync`) that does not know it answers JPEG, read below.
    let path = format!("/frame?device={}&fmt=qoi", percent_encode(device));
    // `Err(None)` from metal-sync means nothing is listening there; any HTTP
    // answer from it, error or not, is the answer — no second opinion.
    let bytes = match fetch_bytes(crate::call::peers::sync_port(), &path) {
        Err(None) => fetch_bytes(port, &path),
        answered => answered,
    };
    let bytes = bytes.map_err(|status| match status {
        Some(409) => FrameError::Ceded,
        _ => FrameError::Unavailable,
    })?;
    if bytes.starts_with(b"qoif") || bytes.starts_with(b"qois") {
        let (width, height, rgba) = crate::qoi::decode(&bytes).ok_or(FrameError::Unavailable)?;
        return Ok((width, height, crate::qoi::rgb_of(&rgba)));
    }
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(&bytes));
    // `decode` answers with the pixels themselves — RGB rows for a
    // 3-component frame. Grayscale masters would repaint a 1-component
    // buffer, so replicate into 3 bytes per pixel and keep one stride.
    let pixels = decoder.decode().map_err(|_| FrameError::Unavailable)?;
    let Some(info) = decoder.info() else {
        return Err(FrameError::Unavailable);
    };
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    let rgb = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => pixels
            .iter()
            .copied()
            .flat_map(|y| [y, y, y])
            .collect::<Vec<u8>>(),
        jpeg_decoder::PixelFormat::L16 => return Err(FrameError::Unavailable),
        _ => pixels,
    };
    Ok((width, height, rgb))
}

/// The one blocking GET the mode's metadata reads need (`/frame` and
/// `/monitor`). `metal-web`'s hand-rolled inbound server is the shape this
/// walks backwards: status line, headers, then the body per
/// Content-Length or to EOF (`Connection: close`, always).
///
/// `Err(Some(status))` is a well-formed HTTP response whose status wasn't
/// 200 (409 meaning "ceded" is the one the caller cares to tell apart);
/// `Err(None)` is anything below the HTTP layer — connect failed, a read
/// failed — where there's no status to report.
pub(crate) fn fetch_sync_bytes(path: &str) -> Result<Vec<u8>, Option<u16>> {
    fetch_bytes(crate::call::peers::sync_port(), path)
}

fn fetch_bytes(port: u16, path: &str) -> Result<Vec<u8>, Option<u16>> {
    use std::io::Write as _;

    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|_| None)?;
    let head =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).map_err(|_| None)?;
    let mut reader = BufReader::new(stream);

    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|_| None)?;
    let status = parse_status_line(&status_line);
    if status != Some(200) {
        return Err(status);
    }
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|_| None)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = Vec::new();
    match content_length {
        Some(len) => {
            body.resize(len, 0);
            reader.read_exact(&mut body).map_err(|_| None)?;
        }
        None => {
            reader.read_to_end(&mut body).map_err(|_| None)?;
        }
    }
    Ok(body)
}

/// Pulls the status code out of an HTTP status line (`"HTTP/1.1 409
/// Conflict\r\n"` → `Some(409)`) — its own pure function so `fetch_bytes`'s
/// one parsing decision is unit-testable without a real socket.
fn parse_status_line(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse::<u16>().ok()
}

/// A bare `--monitor` asks the master whose device it should paint: the
/// first source the master's own `/monitor` route lists, the same order
/// `out.html` shows.
fn first_source_device(port: u16) -> Option<String> {
    let bytes = fetch_bytes(port, "/monitor").ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value["sources"]
        .as_array()?
        .first()
        .and_then(|source| source["device"].as_str())
        .map(str::to_string)
}

/// Enough of the percent-encode rules a query value needs: a device id is
/// `[0-9a-z.-]` in practice, but a stray byte cannot corrupt the request
/// line however it arrived through the CLI.
pub(crate) fn percent_encode(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-._".contains(c) {
                c.to_string()
            } else {
                format!("%{:X}", c as u32 % 256)
            }
        })
        .collect()
}

/// Nearest-neighbour scale of the RGB frame into the surface buffer. The
/// fetch is what a 60 Hz projector measures, not the resampling — the
/// cheapest whole-frame map loses nothing that matters.
fn paint_scaled(
    frame_w: u32,
    frame_h: u32,
    rgb: &[u8],
    width: u32,
    height: u32,
    buffer: &mut [u32],
) {
    for y in 0..height {
        let source_y = source_at(y, height, frame_h) as usize;
        for x in 0..width {
            let source_x = source_at(x, width, frame_w) as usize;
            let base = (source_y * frame_w as usize + source_x) * 3;
            if base + 2 >= rgb.len() {
                continue;
            }
            let (r, g, b) = (
                u32::from(rgb[base]),
                u32::from(rgb[base + 1]),
                u32::from(rgb[base + 2]),
            );
            buffer[(y * width + x) as usize] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
}

/// Nearest source coordinate for a target coordinate: the proportional map,
/// ceiling-free.
fn source_at(x: u32, total: u32, source_total: u32) -> u32 {
    ((u64::from(x) * u64::from(source_total)) / u64::from(total)) as u32
}

/// Whether enough time has passed to poll again while ceded — roughly
/// 2 Hz, so a minimized window checks back without hammering the master.
fn poll_due(last: Option<web_time::Instant>, now: web_time::Instant) -> bool {
    match last {
        None => true,
        Some(t) => now.duration_since(t) >= std::time::Duration::from_millis(500),
    }
}

/// A `MonitorHandle` reduced to the fields `choose` needs, so the picking
/// rule can be unit-tested without a live event loop.
#[derive(Debug, Clone)]
struct MonitorInfo {
    name: Option<String>,
    position: (i32, i32),
    size: (u32, u32),
}

impl MonitorInfo {
    fn from_handle(handle: &MonitorHandle) -> MonitorInfo {
        MonitorInfo {
            name: handle.name(),
            position: (handle.position().x, handle.position().y),
            size: (handle.size().width, handle.size().height),
        }
    }
}

/// Case-insensitive index of the monitor named exactly `name`.
fn exact_name(monitors: &[MonitorInfo], name: &str) -> Option<usize> {
    monitors.iter().position(|m| {
        m.name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    })
}

/// Case-insensitive index of the first monitor whose name contains `needle`.
fn name_contains(monitors: &[MonitorInfo], needle: &str) -> Option<usize> {
    let needle = needle.to_ascii_lowercase();
    monitors.iter().position(|m| {
        m.name
            .as_ref()
            .is_some_and(|n| n.to_ascii_lowercase().contains(&needle))
    })
}

/// Whether `display` names some monitor — shared by `choose` and the
/// caller's "no such display" log line, so the two can never disagree.
fn matches_display(monitors: &[MonitorInfo], display: &str) -> bool {
    exact_name(monitors, display).is_some() || name_contains(monitors, display).is_some()
}

/// Picks which monitor the window fills. An explicit `display` (from
/// `--display`) wins outright when it names one, exactly or as a
/// substring; failing that (or absent), the monitor named like `device`
/// wins; failing that, the first monitor that is not a laptop panel (by
/// connector prefix, or by not sitting at the origin when no monitor
/// reports a name); failing that, whichever monitor there is.
fn choose(monitors: &[MonitorInfo], display: Option<&str>, device: &str) -> Option<usize> {
    if let Some(display) = display {
        if let Some(i) = exact_name(monitors, display) {
            return Some(i);
        }
        if let Some(i) = name_contains(monitors, display) {
            return Some(i);
        }
    }

    if let Some(i) = exact_name(monitors, device) {
        return Some(i);
    }

    if monitors.len() > 1 {
        const PANEL_PREFIXES: [&str; 3] = ["edp", "lvds", "dsi"];
        let any_named = monitors.iter().any(|m| m.name.is_some());
        if any_named {
            if let Some(i) = monitors.iter().position(|m| {
                m.name.as_deref().is_some_and(|n| {
                    let lower = n.to_ascii_lowercase();
                    !PANEL_PREFIXES.iter().any(|p| lower.starts_with(p))
                })
            }) {
                return Some(i);
            }
        } else if let Some(i) = monitors.iter().position(|m| m.position != (0, 0)) {
            return Some(i);
        }
        return Some(0);
    }

    if monitors.len() == 1 {
        return Some(0);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, position: (i32, i32)) -> MonitorInfo {
        MonitorInfo {
            name: Some(name.to_string()),
            position,
            size: (1920, 1080),
        }
    }

    fn nameless(position: (i32, i32)) -> MonitorInfo {
        MonitorInfo {
            name: None,
            position,
            size: (1920, 1080),
        }
    }

    #[test]
    fn an_explicit_display_wins() {
        let monitors = [named("eDP-1", (0, 0)), named("HDMI-A-1", (1280, 0))];
        assert_eq!(choose(&monitors, Some("HDMI-A-1"), "eDP-1"), Some(1));
    }

    #[test]
    fn an_explicit_display_matches_a_substring_when_nothing_is_exact() {
        let monitors = [named("eDP-1", (0, 0)), named("HDMI-A-1", (1280, 0))];
        assert_eq!(choose(&monitors, Some("hdmi"), "eDP-1"), Some(1));
    }

    #[test]
    fn the_device_id_names_its_own_monitor() {
        let monitors = [named("eDP-1", (0, 0)), named("HDMI-A-1", (1280, 0))];
        assert_eq!(choose(&monitors, None, "HDMI-A-1"), Some(1));
    }

    #[test]
    fn without_a_hint_the_external_monitor_beats_the_laptop_panel() {
        let monitors = [named("eDP-1", (0, 0)), named("HDMI-A-1", (1280, 0))];
        assert_eq!(choose(&monitors, None, "screen-1"), Some(1));
    }

    #[test]
    fn nameless_monitors_prefer_the_one_not_at_origin() {
        let monitors = [nameless((0, 0)), nameless((1280, 0))];
        assert_eq!(choose(&monitors, None, "screen-1"), Some(1));
    }

    #[test]
    fn a_single_monitor_is_the_answer() {
        let monitors = [named("eDP-1", (0, 0))];
        assert_eq!(choose(&monitors, None, "screen-1"), Some(0));
    }

    #[test]
    fn an_unknown_display_falls_back_to_the_default_rule() {
        let monitors = [named("eDP-1", (0, 0)), named("HDMI-A-1", (1280, 0))];
        assert_eq!(choose(&monitors, Some("DP-2"), "HDMI-A-1"), Some(1));
    }

    #[test]
    fn no_monitors_is_none() {
        let monitors: [MonitorInfo; 0] = [];
        assert_eq!(choose(&monitors, None, "screen-1"), None);
    }

    #[test]
    fn parse_status_line_reads_the_code_out_of_a_409() {
        assert_eq!(parse_status_line("HTTP/1.1 409 Conflict\r\n"), Some(409));
    }

    #[test]
    fn parse_status_line_reads_the_code_out_of_a_200() {
        assert_eq!(parse_status_line("HTTP/1.1 200 OK\r\n"), Some(200));
    }

    #[test]
    fn parse_status_line_rejects_garbage() {
        assert_eq!(parse_status_line("not a status line\r\n"), None);
    }

    #[test]
    fn poll_due_is_always_due_when_never_polled() {
        assert!(poll_due(None, web_time::Instant::now()));
    }

    #[test]
    fn poll_due_is_not_due_right_after_polling() {
        let now = web_time::Instant::now();
        assert!(!poll_due(Some(now), now));
    }

    #[test]
    fn poll_due_is_due_after_the_throttle_window() {
        let last = web_time::Instant::now();
        let now = last + std::time::Duration::from_millis(600);
        assert!(poll_due(Some(last), now));
    }
}
