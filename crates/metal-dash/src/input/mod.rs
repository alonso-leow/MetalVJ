//! O.10b: external inputs → `Trigger` — the shared runtime.
//!
//! Every external input ultimately fires the *same* HTTP route a human (or
//! the AI) uses: a reader thread hears something, then does one loopback
//! `POST /trigger/<id>` (`regla 6` — no second path, no second state
//! machine). This module owns the pieces the readers share: the learn arm,
//! the loopback POST, the OSC reader's thread, and the pure matching helper
//! the readers answer with.
//!
//! **Learn** is in-the-air only: an operator arms a button, the reader sends
//! the first thing it hears with `{"op":"learn","source":…}`, and the route
//! (`call::transmit::trigger`) writes it into the button's `Trigger`. Like
//! `overlays`/`runs`, the arm is never saved — a restart must not resume
//! learning a control nobody is touching.

use std::sync::{Arc, LazyLock, Mutex};

/// Which trigger is waiting to learn its source, and which kind ("osc"/
/// "key"/"midi"). In the air only, never persisted — see the module note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearnArm {
    pub trigger: String,
    pub kind: String,
}

/// The one learn arm this process has, mirroring `overlays`/`runs` (in the
/// air only, one per process). `LazyLock` + `Mutex` because a reader thread
/// on one side and the HTTP route on the other both reach it.
pub fn learn() -> &'static Mutex<Option<LearnArm>> {
    static ARM: LazyLock<Mutex<Option<LearnArm>>> = LazyLock::new(|| Mutex::new(None));
    &ARM
}

/// One message a controller sent, as the control surface reads it: the port
/// it came in on, the control id with its channel (`note:60@1`, `cc:21@1` —
/// [`metal_midi::same_control`] matches it against a bare binding) and what
/// it did.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceEvent {
    pub port: String,
    pub input: String,
    pub edge: metal_midi::Edge,
}

/// The most events kept while nothing drains them (the grid not painted): a
/// knob swept for minutes must not grow the queue without limit.
const SURFACE_QUEUE: usize = 512;

fn surface_queue() -> &'static Mutex<std::collections::VecDeque<SurfaceEvent>> {
    static Q: LazyLock<Mutex<std::collections::VecDeque<SurfaceEvent>>> = LazyLock::new(Default::default);
    &Q
}

type Waker = Box<dyn Fn() + Send + Sync>;

fn waker() -> &'static Mutex<Option<Waker>> {
    static W: LazyLock<Mutex<Option<Waker>>> = LazyLock::new(|| Mutex::new(None));
    &W
}

/// Queues one event for the surface and wakes the window, so a key pressed on
/// an idle app paints at once.
pub fn push_surface(event: SurfaceEvent) {
    if let Ok(mut q) = surface_queue().lock() {
        if q.len() >= SURFACE_QUEUE {
            q.pop_front();
        }
        q.push_back(event);
    }
    if let Some(wake) = waker().lock().ok().as_ref().and_then(|w| w.as_ref()) {
        wake();
    }
}

/// Everything queued since the last call, oldest first.
pub fn drain_surface() -> Vec<SurfaceEvent> {
    surface_queue().lock().map(|mut q| q.drain(..).collect()).unwrap_or_default()
}

/// Whether anything is queued for the surface.
pub fn surface_pending() -> bool {
    surface_queue().lock().map(|q| !q.is_empty()).unwrap_or(false)
}

/// How the window asks to be repainted; the shell sets it once.
pub fn set_surface_waker(wake: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut w) = waker().lock() {
        *w = Some(Box::new(wake));
    }
}

fn ports_list() -> &'static Mutex<Vec<String>> {
    static P: LazyLock<Mutex<Vec<String>>> = LazyLock::new(Default::default);
    &P
}

/// The MIDI ports the reader has open (empty without the `midi` feature).
pub fn midi_ports() -> Vec<String> {
    ports_list().lock().map(|p| p.clone()).unwrap_or_default()
}

/// `true` when this build can read MIDI at all.
pub fn midi_built() -> bool {
    cfg!(feature = "midi")
}

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
fn set_midi_ports(names: Vec<String>) {
    if let Ok(mut p) = ports_list().lock() {
        *p = names;
    }
}

/// Minimal loopback HTTP/1.1 `POST` of a JSON body to
/// `127.0.0.1:<port>/trigger/<id>`.
///
/// Std-only (`TcpStream`), no HTTP crate. Connects with a short timeout so a
/// dead app cannot hang a reader thread, and reads/discards the response
/// (the route's own reply is for a human, not the reader). A read that times
/// out is not an error: the request was sent, which is all a reader owes.
pub fn post_trigger(
    host: &str,
    port: u16,
    id: &str,
    body: &serde_json::Value,
) -> std::io::Result<()> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let payload = body.to_string();
    let request = format!(
        "POST /trigger/{id} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no address"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500))?;
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    stream.write_all(request.as_bytes())?;
    let mut buf = [0u8; 1024];
    let mut seen = 0usize;
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                seen += n;
                // The reply is discarded; this bound only stops a chatty
                // peer from growing the buffer without limit.
                if seen > 1 << 20 {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Spawns the OSC reader thread. A UDP port that cannot bind logs and leaves
/// the rest of the app running (the reader is a convenience, not a
/// precondition) — the thread itself is what [`osc::run`] does.
pub fn spawn_osc_reader(
    shared: Arc<Mutex<crate::Shared>>,
    host: String,
    http_port: u16,
    udp_port: u16,
) {
    // The browser has no threads and no UDP socket: the reader is native only.
    #[cfg(target_arch = "wasm32")]
    let _ = (shared, host, http_port, udp_port);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = std::thread::Builder::new()
        .name("metal-dash osc".into())
        .spawn(move || osc::run(shared, host, http_port, udp_port));
}

pub mod osc;

/// The MIDI reader (`src/input/midi.rs`), behind `--features midi`: `midir`
/// links the system ALSA sequencer through `alsa-sys`, so the default build
/// must not need ALSA headers. It is the twin of [`spawn_osc_reader`] — the
/// thread itself is what [`midi::run`] does.
#[cfg(feature = "midi")]
pub mod midi;

/// The pure half of the OSC reader: given a heard address and the app's
/// current state, whose triggers should it fire?
///
/// Returns `(ids, learn)`:
/// - `learn` is `Some(trigger)` when the learn arm is armed for `"osc"` —
///   the reader posts the learned source there and clears it (`regla 4`:
///   the arm is a read of the one learn state).
/// - otherwise `ids` is every active trigger (`state.sources` and the built
///   in `state.preview`) whose `TriggerSource::Osc.addr` matches exactly.
pub fn osc_targets(shared: &crate::Shared, addr: &str) -> (Vec<String>, Option<String>) {
    if let Some(arm) = learn().lock().ok().and_then(|g| g.clone()) {
        if arm.kind == "osc" {
            return (Vec::new(), Some(arm.trigger));
        }
    }
    let mut ids = Vec::new();
    for dev in shared.state.sources.values() {
        for t in &dev.triggers {
            if t.source.matches_osc(addr) {
                ids.push(t.id.clone());
            }
        }
    }
    for t in &shared.state.preview.triggers {
        if t.source.matches_osc(addr) {
            ids.push(t.id.clone());
        }
    }
    (ids, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// The arm is process-wide; every test that touches it starts from a
    /// known state so the suite is order-independent.
    fn reset_learn() {
        *learn().lock().expect("learn lock") = None;
    }

    fn shared_with_probe() -> crate::Shared {
        let paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(crate::output::Output::new("probe", "Probe", 64, 64));
        state
            .sources
            .entry("probe".to_string())
            .or_insert_with(crate::layers::Device::default);
        let sim = crate::listeners::Listeners::stand_in();
        crate::Shared { state, paint, sim }
    }

    /// The "fake reader" seam: a real `TcpListener` accepting one loopback
    /// request. The reader must send exactly the route it means, with the
    /// body as JSON.
    #[test]
    fn post_trigger_sends_the_route_line_and_json_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut got = Vec::new();
            let mut buf = [0u8; 512];
            loop {
                let n = stream.read(&mut buf).expect("read");
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                if got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let text = String::from_utf8_lossy(&got);
                    if let Some(at) = text.find("\r\n\r\n") {
                        let head = &text[..at];
                        let clen: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        while got.len() - (at + 4) < clen {
                            let n = stream.read(&mut buf).expect("read body");
                            if n == 0 {
                                break;
                            }
                            got.extend_from_slice(&buf[..n]);
                        }
                    }
                    break;
                }
            }
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            got
        });

        let body = serde_json::json!({ "op": "learn", "source": { "source": "osc", "addr": "/pad/1" } });
        post_trigger("127.0.0.1", port, "pedal", &body).expect("the post goes out");
        let got = server.join().expect("server thread");
        let text = String::from_utf8_lossy(&got);
        assert!(
            text.starts_with("POST /trigger/pedal HTTP/1.1\r\n"),
            "wrong request line: {text}"
        );
        assert!(text.contains("Content-Type: application/json"));
        assert!(text.contains(&body.to_string()), "body did not arrive");
    }

    /// A trigger with `Osc{addr}` matches exactly its address, and a trigger
    /// carrying a different address does not.
    #[test]
    fn osc_targets_matches_the_address_and_nothing_else() {
        reset_learn();
        let mut shared = shared_with_probe();
        shared
            .state
            .sources
            .get_mut("probe")
            .unwrap()
            .triggers
            .push(crate::trigger::Trigger::new_osc(
                "pad".to_string(),
                "overlay-drop".to_string(),
                "/pad/1".to_string(),
            ));

        let (hit, learn) = osc_targets(&shared, "/pad/1");
        assert_eq!(hit, vec!["pad".to_string()]);
        assert_eq!(learn, None);

        let (miss, _) = osc_targets(&shared, "/pad/2");
        assert!(miss.is_empty(), "a different address must not match");
    }

    /// With the arm set for `"osc"`, the heard address goes to the armed
    /// trigger instead of the active matches.
    #[test]
    fn osc_targets_prefers_the_learn_arm() {
        reset_learn();
        *learn().lock().unwrap() = Some(LearnArm {
            trigger: "pedal".to_string(),
            kind: "osc".to_string(),
        });
        let shared = shared_with_probe();
        let (ids, learn_target) = osc_targets(&shared, "/anything");
        assert!(ids.is_empty());
        assert_eq!(learn_target.as_deref(), Some("pedal"));
        reset_learn();
    }
}
