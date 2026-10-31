//! The pairing overlay (plan §5 item 6) and the chrome's sync indicator.
//!
//! Pairing is `fw/pair/1` (`metal-sync/src/pair.rs`): the machine that is
//! already in the club shows a six-digit code on its own screen, and the
//! operator reads it off and types it on the new machine. This overlay is
//! where both halves happen from the shell: **nearby** lists what `metal-sync`
//! discovered on the LAN, **pair** sends a request, the code box confirms it
//! once the other screen shows the code, and a request that arrives here
//! shows its code to read out.
//!
//! **Regla 6.** Every read and write goes through the door: `GET
//! /sync/{status,peers,discover,pair}` and `POST /sync/pair/*`
//! (`call::sync`), the same routes the `ia` tools `sync_status`/`sync_peers`
//! read. The cache below is what the chrome indicator and the Dispositivos
//! section paint from, refreshed at most once a second, so the door is not
//! asked on every frame.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use egui::{Id, Response, RichText, Sense, TextEdit, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{panel_frame, section_heading};
use crate::shell::i18n::{tr, trf};

/// Stable id salt for the overlay's widgets.
pub const ID: &str = "shell/panel/pairing";

/// How often the cache asks the door again.
const POLL_EVERY: Duration = Duration::from_secs(1);

/// What the chrome says about sync, read off the last poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Summary {
    /// `metal-sync` is not running on this machine.
    NotRunning,
    /// Running, with no peer in the club yet.
    Unpaired,
    /// Running, with this many peers in the club.
    Paired(usize),
}

/// What a pairing POST was for, so its answer can be applied when it lands.
enum Action {
    Request(String),
    Confirm,
    Reject,
}

/// The pairing POSTs' thread (2026-10-09). A `metal-sync` dial can wait the
/// door's whole 20 s, so the overlay never makes one on the frame: it hands
/// the POST here, paints a spinner, and takes the answer in on a later frame
/// ([`PanelState::collect`]). The worker owns its own handle to the door, the
/// same way [`super::super::frames::Fetcher`] does for the previews.
pub struct PairWorker {
    tx: Sender<Job>,
    /// The sync reads (`status`, `peers`, `discover`, `pair`), one poll per
    /// message, each answered on the channel it carries.
    polls: Sender<Sender<[Value; 5]>>,
}

/// One POST: the route, its body, and the channel its answer goes back on.
type Job = (String, Value, Sender<Result<(), String>>);

impl PairWorker {
    /// Starts the worker around `door`. A worker that cannot spawn leaves a
    /// `PairWorker` whose jobs never answer; the overlay then reads the
    /// disconnected channel as a refusal, never as a hang.
    pub fn spawn<D: Call + Clone + Send + 'static>(door: D) -> PairWorker {
        let (tx, rx) = mpsc::channel::<Job>();
        let poll_door = door.clone();
        let _ = thread::Builder::new().name("metal-dash pairing".into()).spawn(move || {
            while let Ok((path, body, back)) = rx.recv() {
                let result = door.json("POST", &path, Some(body)).map(drop).map_err(|(_, msg)| msg);
                let _ = back.send(result);
            }
        });
        // The reads have their own thread: `metal-sync` can take seconds to
        // refuse, and the frame must not wait for it.
        let (polls, poll_rx) = mpsc::channel::<Sender<[Value; 5]>>();
        let _ = thread::Builder::new().name("metal-dash sync poll".into()).spawn(move || {
            while let Ok(back) = poll_rx.recv() {
                let read = |path: &str| poll_door.json("GET", path, None).unwrap_or_else(|_| json!({}));
                let reads = [
                    read("/sync/status"),
                    read("/sync/peers"),
                    read("/sync/discover"),
                    read("/sync/pair"),
                    read("/sync/stats"),
                ];
                let _ = back.send(reads);
            }
        });
        PairWorker { tx, polls }
    }

    /// Hands one POST to the worker; the answer comes back on the returned
    /// channel. A dead worker drops the job with its sender, so the answer
    /// reads as disconnected.
    fn start(&self, path: &str, body: Value) -> Receiver<Result<(), String>> {
        let (back, answer) = mpsc::channel();
        let _ = self.tx.send((path.to_string(), body, back));
        answer
    }
}

/// The cache: the last answer of each sync read, and the code box's text.
#[derive(Default)]
pub struct PanelState {
    /// Did the last poll reach `metal-sync`?
    running: bool,
    /// This machine's name in the club.
    name: String,
    /// `GET /sync/peers`' list, as the route answers it.
    peers: Vec<Value>,
    /// `GET /sync/discover`'s list: machines seen on the LAN.
    nearby: Vec<Value>,
    /// Requests from other machines this one is showing a code for.
    pending: Vec<Value>,
    /// The node id of the request this machine sent, waiting for its code.
    outgoing: Option<String>,
    /// The code box: the six digits read off the other machine.
    code: String,
    /// The last action's refusal, in `metal-sync`'s own words.
    error: Option<String>,
    /// When the cache last asked the door. `None` means "ask on the next
    /// frame" (set after every action, so its result shows at once).
    last_poll: Option<Instant>,
    /// The worker that runs the POSTs, when the shell has one. Without it
    /// (the kittest harness) a POST runs inline, as before.
    worker: Option<PairWorker>,
    /// The POST on the worker, and what its answer is for.
    in_flight: Option<(Action, Receiver<Result<(), String>>)>,
    /// The sync reads on the worker, when one is set and a poll is out.
    poll_in_flight: Option<Receiver<[Value; 5]>>,
    /// `GET /sync/stats`' peers' self-reports, as `metal-sync` keeps them.
    stats: Vec<Value>,
}

impl PanelState {
    /// The chrome's summary, off the last poll.
    pub fn summary(&self) -> Summary {
        if !self.running {
            Summary::NotRunning
        } else if self.peers.is_empty() {
            Summary::Unpaired
        } else {
            Summary::Paired(self.peers.len())
        }
    }

    /// Hands the POSTs to `worker` from now on.
    pub fn set_worker(&mut self, worker: PairWorker) {
        self.worker = Some(worker);
    }

    /// Whether a POST is on its way to `metal-sync`: the overlay paints a
    /// spinner and holds its buttons until it lands.
    pub fn busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Asks the door for every sync read, if the last ask is older than
    /// [`POLL_EVERY`]. Called once per frame from `Shell::ui`, after the
    /// worker's answer (if one landed) is taken in.
    pub fn refresh_due(&mut self, door: &dyn Call, now: Instant) {
        self.collect();
        self.collect_poll(now);
        let due = match self.last_poll {
            None => true,
            Some(then) => now.duration_since(then) >= POLL_EVERY,
        };
        if !due {
            return;
        }
        // With a worker the reads run there and land on a later frame; the
        // kittest harness, which has none, reads inline.
        let sent = match (&self.worker, &self.poll_in_flight) {
            (Some(worker), None) => {
                let (back, answer) = mpsc::channel();
                worker.polls.send(back).ok().map(|_| answer)
            }
            _ => None,
        };
        match sent {
            Some(answer) => {
                self.poll_in_flight = Some(answer);
                self.last_poll = Some(now);
            }
            None if self.worker.is_none() => self.refresh(door, now),
            None => {}
        }
    }

    /// Takes the sync reads in, if the worker has answered.
    fn collect_poll(&mut self, now: Instant) {
        let Some(answer) = self.poll_in_flight.take() else { return };
        match answer.try_recv() {
            Ok([status, peers, nearby, pair, stats]) => {
                self.absorb(&status, &peers, &nearby, &pair);
                self.absorb_stats(&stats);
                self.last_poll = Some(now);
            }
            Err(TryRecvError::Empty) => self.poll_in_flight = Some(answer),
            Err(TryRecvError::Disconnected) => {}
        }
    }

    /// Asks the door for every sync read now. A read the door refuses or
    /// does not answer counts as "not running", never as a stale answer.
    pub fn refresh(&mut self, door: &dyn Call, now: Instant) {
        self.last_poll = Some(now);
        let read = |path: &str| door.json("GET", path, None).unwrap_or_else(|_| json!({}));
        let status = read("/sync/status");
        let peers = read("/sync/peers");
        let nearby = read("/sync/discover");
        let pair = read("/sync/pair");
        let stats = read("/sync/stats");
        self.absorb(&status, &peers, &nearby, &pair);
        self.absorb_stats(&stats);
    }

    /// Takes `GET /sync/stats` in. A reply with no `peers` list leaves none.
    fn absorb_stats(&mut self, stats: &Value) {
        self.stats = list(stats, "peers");
    }

    /// Takes the four replies into the cache. Pure, so the mapping from the
    /// wire shape is tested without a door.
    fn absorb(&mut self, status: &Value, peers: &Value, nearby: &Value, pair: &Value) {
        self.running = status["running"].as_bool().unwrap_or(false);
        self.name = status["name"].as_str().unwrap_or_default().to_string();
        self.peers = list(peers, "peers");
        self.nearby = list(nearby, "nearby");
        self.pending = list(pair, "pending");
        self.outgoing = pair["outgoing"]["node_id"].as_str().map(str::to_string);
    }

    /// Sends one pairing POST: to the worker when there is one (the frame
    /// goes on at once and [`collect`](Self::collect) takes the answer in),
    /// otherwise through `door` right here. A POST already in flight is
    /// not joined by another.
    fn act(&mut self, door: &dyn Call, action: Action, path: &str, body: Value) {
        if self.in_flight.is_some() {
            return;
        }
        match &self.worker {
            Some(worker) => self.in_flight = Some((action, worker.start(path, body))),
            None => {
                let result = door.json("POST", path, Some(body)).map(drop).map_err(|(_, msg)| msg);
                self.finish(action, result);
            }
        }
    }

    /// Takes the worker's answer in, if it has landed. A worker that went
    /// away reads as a refusal.
    fn collect(&mut self) {
        let Some((action, answer)) = self.in_flight.take() else { return };
        match answer.try_recv() {
            Ok(result) => self.finish(action, result),
            Err(TryRecvError::Empty) => self.in_flight = Some((action, answer)),
            Err(TryRecvError::Disconnected) => {
                self.finish(action, Err(tr("the pairing worker stopped").to_string()))
            }
        }
    }

    /// Applies one POST's answer: the state it implies on success, the
    /// refusal in `metal-sync`'s own words otherwise. The next frame polls, so
    /// the result is on screen at once.
    fn finish(&mut self, action: Action, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.error = None;
                match action {
                    Action::Request(node_id) => {
                        self.outgoing = Some(node_id);
                        self.code.clear();
                    }
                    Action::Confirm => {
                        self.code.clear();
                        self.outgoing = None;
                    }
                    Action::Reject => {}
                }
            }
            Err(msg) => self.error = Some(error_text(&msg)),
        }
        self.last_poll = None;
    }

    /// Start a request to a machine seen nearby.
    fn request(&mut self, door: &dyn Call, node_id: &str) {
        let action = Action::Request(node_id.to_string());
        self.act(door, action, "/sync/pair/request", json!({ "node_id": node_id }));
    }

    /// Confirm the code typed here against the request waiting on it.
    fn confirm(&mut self, door: &dyn Call) {
        if !is_code(&self.code) {
            return;
        }
        let code = self.code.clone();
        self.act(door, Action::Confirm, "/sync/pair/confirm", json!({ "code": code }));
    }

    /// Refuse a request shown on this machine.
    fn reject(&mut self, door: &dyn Call, node_id: &str) {
        self.act(door, Action::Reject, "/sync/pair/reject", json!({ "node_id": node_id }));
    }
}

/// A pairing code is six decimal digits (`metal-sync`'s `pair::new_code`).
/// Checked here as well as at the route, so the button stays honest.
fn is_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

/// The array under `key`, or nothing: a reply without it is an empty list.
fn list(reply: &Value, key: &str) -> Vec<Value> {
    reply[key].as_array().cloned().unwrap_or_default()
}

/// The door's error text is the route's JSON body (`{"error": "…"}`); the
/// human line is its `error`, and anything else is shown as it came.
fn error_text(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| raw.trim().to_string())
}

/// The first eight characters of a node id — enough to tell two machines
/// apart on a screen, never the whole key.
fn short(node_id: &str) -> String {
    node_id.chars().take(8).collect()
}

/// The name a nearby machine announces, or its short id when it has none.
fn name_of(nearby: &[Value], node_id: &str) -> String {
    nearby
        .iter()
        .find(|m| m["node_id"].as_str() == Some(node_id))
        .and_then(|m| m["name"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| short(node_id))
}

/// The chrome's indicator text for a summary. Pure, so the three states are
/// tested without a window.
pub fn chip_text(summary: Summary) -> String {
    match summary {
        Summary::NotRunning => tr("sync off").to_string(),
        Summary::Unpaired => tr("unpaired").to_string(),
        Summary::Paired(n) => trf("paired · {n}", &[("n", &n)]),
    }
}

/// The sync indicator in the header: the chip's text, clickable to open the
/// pairing overlay. Returns whether it was clicked this frame.
pub fn chip(ui: &mut Ui, theme: &Theme, state: &PanelState) -> bool {
    let summary = state.summary();
    let color = match summary {
        Summary::NotRunning => theme.warn,
        Summary::Unpaired => theme.ink_weak,
        Summary::Paired(_) => theme.phosphor_lit,
    };
    ui.push_id(Id::new("shell/header/pairing"), |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(chip_text(summary))
                    .font(theme.font_small.clone())
                    .color(color),
            )
            .sense(Sense::click()),
        )
        .on_hover_text(tr("pair a device"))
        .clicked()
    })
    .inner
}

/// The overlay's whole content: the nearby machines, the code box for a
/// request this machine sent, and the codes this machine shows for requests
/// that arrived. The surrounding `Area` is the caller's (`Shell::ui`).
pub fn paint(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) -> Response {
    ui.push_id(Id::new(ID), |ui| {
        panel_frame(theme)
            .show(ui, |ui| {
                section_heading(ui, theme, tr("pair a device"));
                if !state.running {
                    ui.label(text(tr("metal-sync is not running on this machine"), theme.warn, theme));
                    return;
                }
                let busy = state.busy();
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(text(tr("working…"), theme.ink_weak, theme));
                    });
                }

                // Requests that arrived: this machine shows the code, the other
                // operator types it there.
                for req in state.pending.clone() {
                    let from = req["name"].as_str().unwrap_or("?").to_string();
                    let code = req["code"].as_str().unwrap_or("?").to_string();
                    let node = req["node_id"].as_str().unwrap_or_default().to_string();
                    ui.horizontal_wrapped(|ui| {
                        let line = trf("{from} wants to join: code {code}", &[("from", &from), ("code", &code)]);
                        ui.label(text(&line, theme.phosphor_lit, theme));
                        if ui.add_enabled(!busy, egui::Button::new(tr("reject"))).clicked() {
                            state.reject(door, &node);
                        }
                    });
                }

                // A request this machine sent: the code is on the other screen.
                if let Some(node) = state.outgoing.clone() {
                    let who = name_of(&state.nearby, &node);
                    let line = trf("type the code shown on {who}", &[("who", &who)]);
                    ui.label(text(&line, theme.ink, theme));
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut state.code)
                                .hint_text("000000")
                                .desired_width(theme.region_min.x * 0.3),
                        );
                        let ready = is_code(&state.code) && !busy;
                        if ui.add_enabled(ready, egui::Button::new(tr("confirm"))).clicked() {
                            state.confirm(door);
                        }
                    });
                }

                // The club's neighbours on the LAN, with a request button each.
                ui.add_space(theme.item_gap.y);
                ui.label(text(tr("nearby"), theme.ink_weak, theme));
                if state.nearby.is_empty() {
                    ui.label(text(tr("no machines seen on the network"), theme.ink_faint, theme));
                }
                for machine in state.nearby.clone() {
                    let name = machine["name"].as_str().unwrap_or("?").to_string();
                    let node = machine["node_id"].as_str().unwrap_or_default().to_string();
                    let member = machine["member"].as_bool().unwrap_or(false);
                    ui.horizontal(|ui| {
                        ui.label(text(&name, theme.ink, theme));
                        ui.label(text(&short(&node), theme.ink_faint, theme));
                        if member {
                            ui.label(text(tr("paired"), theme.ok, theme));
                        } else if ui
                            .add_enabled(state.outgoing.is_none() && !busy, egui::Button::new(tr("pair")))
                            .clicked()
                        {
                            state.request(door, &node);
                        }
                    });
                }

                if let Some(err) = state.error.clone() {
                    ui.label(text(&err, theme.warn, theme));
                }
                ui.add_space(theme.item_gap.y);
            })
            .response
    })
    .inner
}

/// The Dispositivos page (settings): this machine's sync state and the
/// club's members, read off the same cache the overlay uses. Read-only;
/// pairing happens in the overlay.
pub fn paint_devices(ui: &mut Ui, theme: &Theme, state: &PanelState) {
    if state.summary() == Summary::NotRunning {
        ui.label(text(tr("metal-sync is not running on this machine"), theme.warn, theme));
        return;
    }
    if state.peers.is_empty() {
        ui.label(text(
            tr("no paired devices yet: pair one from the sync indicator in the top bar"),
            theme.ink_weak,
            theme,
        ));
        return;
    }
    for peer in &state.peers {
        let name = peer["name"].as_str().unwrap_or("?");
        let node = peer["node_id"].as_str().unwrap_or_default();
        let online = peer["online"].as_bool().unwrap_or(false);
        ui.horizontal(|ui| {
            ui.label(text(name, theme.ink, theme));
            let (word, color) = if online {
                (tr("online"), theme.ok)
            } else {
                (tr("offline"), theme.ink_faint)
            };
            ui.label(text(word, color, theme));
            ui.label(text(&short(node), theme.ink_faint, theme));
        });
        // The peer's own report, read-only: stale is shown as stale with its
        // last values, a missing report or reading as "—", never as zero.
        let stat = stat_for(&state.stats, name);
        ui.horizontal(|ui| {
            let (word, color) = match stat.and_then(|v| v["status"].as_str()) {
                Some("fresh") => (tr("fresh"), theme.ok),
                Some("stale") => (tr("stale"), theme.warn),
                _ => (tr("no report yet"), theme.ink_faint),
            };
            ui.label(text(word, color, theme));
            if let Some(stat) = stat {
                ui.label(text(&readings_text(stat), theme.ink_weak, theme));
            }
        });
    }
}

/// The stats entry for the peer called `name`, if `metal-sync` has heard from it.
fn stat_for<'a>(stats: &'a [Value], name: &str) -> Option<&'a Value> {
    stats.iter().find(|v| v["name"].as_str() == Some(name))
}

/// One reading as the panel shows it: its number with a unit, or "—" when the
/// machine could not read it.
fn reading(stat: &Value, key: &str, unit: &str) -> String {
    match stat[key].as_f64() {
        Some(x) => format!("{x:.0}{unit}"),
        None => "—".to_string(),
    }
}

/// The readings line under a peer: CPU, temperature, paint backend and frame
/// rate, each "—" when unknown.
pub fn readings_text(stat: &Value) -> String {
    let backend = stat["paint_backend"].as_str().unwrap_or("—");
    format!(
        "{} {} · {} {} · {} {} · {} {}",
        tr("CPU"),
        reading(stat, "cpu_pct", "%"),
        tr("temp"),
        reading(stat, "temp_c", " °C"),
        tr("paint"),
        backend,
        tr("fps"),
        reading(stat, "fps_paint", ""),
    )
}

/// One small label in the theme's body face, with a colour.
fn text(s: &str, color: egui::Color32, theme: &Theme) -> RichText {
    RichText::new(s).font(theme.font_small.clone()).color(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn peers(n: usize) -> Vec<Value> {
        (0..n).map(|i| json!({ "name": format!("p{i}"), "node_id": "abcdef0123", "online": true })).collect()
    }

    /// Not running beats everything; running with no peers is unpaired; the
    /// count is the peers' count.
    #[test]
    fn the_summary_follows_the_last_poll() {
        let mut s = PanelState::default();
        assert_eq!(s.summary(), Summary::NotRunning);
        s.absorb(&json!({ "running": true, "name": "sala" }), &json!({ "peers": [] }), &json!({}), &json!({}));
        assert_eq!(s.summary(), Summary::Unpaired);
        assert_eq!(s.name, "sala");
        s.absorb(&json!({ "running": true }), &json!({ "peers": peers(2) }), &json!({}), &json!({}));
        assert_eq!(s.summary(), Summary::Paired(2));
        s.absorb(&json!({ "running": false }), &json!({ "running": false, "peers": [] }), &json!({}), &json!({}));
        assert_eq!(s.summary(), Summary::NotRunning);
    }

    #[test]
    fn the_chip_says_what_the_summary_says() {
        assert_eq!(chip_text(Summary::NotRunning), "sync off");
        assert_eq!(chip_text(Summary::Unpaired), "unpaired");
        assert_eq!(chip_text(Summary::Paired(3)), "paired · 3");
    }

    /// The pair half of the poll: a request shown here carries its code, and
    /// the outgoing one is the node id `GET /sync/pair` names.
    #[test]
    fn the_pair_reply_fills_pending_and_outgoing() {
        let mut s = PanelState::default();
        let pair = json!({
            "running": true,
            "pending": [{ "node_id": "n1", "name": "cocina", "code": "048213" }],
            "outgoing": { "node_id": "n2" },
        });
        s.absorb(&json!({ "running": true }), &json!({}), &json!({}), &pair);
        assert_eq!(s.pending.len(), 1);
        assert_eq!(s.pending[0]["code"], "048213");
        assert_eq!(s.outgoing.as_deref(), Some("n2"));

        s.absorb(&json!({ "running": true }), &json!({}), &json!({}), &json!({ "pending": [], "outgoing": null }));
        assert!(s.outgoing.is_none());
    }

    #[test]
    fn a_code_is_exactly_six_digits() {
        assert!(is_code("048213"));
        assert!(!is_code("04821"));
        assert!(!is_code("04821x"));
        assert!(!is_code(""));
    }

    /// The door's refusal is the route's JSON body; the panel shows its
    /// `error` line, and a non-JSON message as it came.
    #[test]
    fn a_refusal_shows_fw_syncs_own_words() {
        assert_eq!(error_text(r#"{"error":"wrong code"}"#), "wrong code");
        assert_eq!(error_text("  plain text  "), "plain text");
    }

    #[test]
    fn a_node_is_named_by_its_announced_name_or_its_short_id() {
        let nearby = vec![json!({ "node_id": "abcdef0123456789", "name": "sala" })];
        assert_eq!(name_of(&nearby, "abcdef0123456789"), "sala");
        assert_eq!(name_of(&nearby, "zzzzzzzzzzzz"), "zzzzzzzz");
    }

    /// A door that cannot answer (no route, a refusal) is "not running",
    /// never a stale answer carried over from a poll that did succeed.
    /// A door whose every POST waits `delay` and answers `fail_path` with a
    /// refusal, as `metal-sync` does for a wrong code.
    #[derive(Clone)]
    struct SlowDoor {
        delay: Duration,
        fail_path: &'static str,
    }

    impl Call for SlowDoor {
        fn call(&self, _method: &str, path: &str, _params: crate::call::Params) -> Option<crate::call::Reply> {
            thread::sleep(self.delay);
            let (status, body) = if path == self.fail_path {
                (409, json!({ "error": "wrong code" }))
            } else {
                (200, json!({}))
            };
            Some(crate::call::Reply { status, kind: "application/json", body: body.to_string().into_bytes() })
        }
    }

    /// Waits, bounded, for the worker's answer to be taken in.
    fn settle(s: &mut PanelState) {
        let until = Instant::now() + Duration::from_secs(5);
        while s.busy() && Instant::now() < until {
            s.collect();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!s.busy(), "the worker never answered");
    }

    /// A pairing POST never holds the frame: `request` returns at once with
    /// the POST in flight, and the answer lands on a later frame.
    #[test]
    fn a_pairing_post_runs_off_the_frame() {
        let door = SlowDoor { delay: Duration::from_millis(400), fail_path: "" };
        let mut s = PanelState::default();
        s.set_worker(PairWorker::spawn(SlowDoor { delay: Duration::from_millis(400), fail_path: "" }));

        let started = Instant::now();
        s.request(&door, "n2");
        assert!(started.elapsed() < Duration::from_millis(200), "the frame waited on the POST");
        assert!(s.busy(), "the overlay should show the spinner while the POST is out");
        assert!(s.outgoing.is_none(), "nothing is applied before the answer");

        settle(&mut s);
        assert_eq!(s.outgoing.as_deref(), Some("n2"));
        assert!(s.error.is_none());
    }

    /// A refusal from `metal-sync` comes back through the worker as its own words.
    #[test]
    fn a_refused_confirm_on_the_worker_shows_fw_syncs_words() {
        let mut s = PanelState::default();
        s.set_worker(PairWorker::spawn(SlowDoor {
            delay: Duration::from_millis(10),
            fail_path: "/sync/pair/confirm",
        }));
        s.outgoing = Some("n2".into());
        s.code = "123456".into();
        s.confirm(&SlowDoor { delay: Duration::ZERO, fail_path: "" });
        assert!(s.busy());
        settle(&mut s);
        assert_eq!(s.error.as_deref(), Some("wrong code"));
        assert_eq!(s.outgoing.as_deref(), Some("n2"), "a refused confirm leaves the request waiting");
        assert_eq!(s.code, "123456", "and keeps the typed code");
    }

    /// A second POST is not joined to one already on its way.
    #[test]
    fn one_pairing_post_at_a_time() {
        let door = SlowDoor { delay: Duration::from_millis(200), fail_path: "" };
        let mut s = PanelState::default();
        s.set_worker(PairWorker::spawn(SlowDoor { delay: Duration::from_millis(200), fail_path: "" }));
        s.request(&door, "n2");
        s.reject(&door, "n3");
        settle(&mut s);
        assert!(s.error.is_none());
        assert_eq!(s.outgoing.as_deref(), Some("n2"));
    }

    #[test]
    fn an_unanswering_door_reads_as_not_running() {
        struct Down;
        impl Call for Down {
            fn call(&self, _: &str, _: &str, _: crate::call::Params) -> Option<crate::call::Reply> {
                None
            }
        }
        let mut s = PanelState::default();
        s.absorb(&json!({ "running": true }), &json!({ "peers": peers(1) }), &json!({}), &json!({}));
        assert_eq!(s.summary(), Summary::Paired(1));
        s.refresh(&Down, Instant::now());
        assert_eq!(s.summary(), Summary::NotRunning);
    }

    /// A peer whose report is stale is painted stale, with its last values; a
    /// reading it never sent reads "—".
    #[test]
    fn a_stale_peer_paints_stale_and_an_unknown_reading_as_a_dash() {
        use egui_kittest::{kittest::Queryable, Harness};
        let mut state = PanelState::default();
        state.absorb(
            &json!({ "running": true, "name": "sala" }),
            &json!({ "peers": [{ "name": "cocina", "node_id": "abcdef0123", "online": true }] }),
            &json!({}),
            &json!({}),
        );
        state.absorb_stats(&json!({ "peers": [{
            "name": "cocina", "status": "stale", "age_s": 31.0,
            "cpu_pct": 12.0, "gpu_pct": null, "temp_c": null,
            "paint_backend": "metal", "fps_paint": 59.0, "uptime_s": 900,
        }] }));
        let theme = Theme::phosphor();
        let mut h = Harness::builder()
            .with_size(egui::Vec2 { x: 640.0, y: 400.0 })
            .build_ui_state(
                |ui, s: &mut PanelState| paint_devices(ui, &theme, s),
                state,
            );
        theme.apply(&h.ctx);
        h.run();
        assert!(h.query_by_label("stale").is_some(), "the peer paints as stale");
        assert!(h.query_by_label_contains("12%").is_some(), "its last CPU value stays shown");
    }

    #[test]
    fn readings_show_a_dash_for_every_value_the_peer_never_sent() {
        let stat = json!({ "cpu_pct": 7.0, "temp_c": null, "paint_backend": null, "fps_paint": null });
        let line = readings_text(&stat);
        assert!(line.contains("7%"), "{line}");
        assert!(!line.contains(" 0"), "an unknown value is never zero: {line}");
        assert_eq!(stat_for(&[stat], "nobody"), None);
    }
}
