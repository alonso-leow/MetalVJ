//! 2026-10-09: a song this node has no audio for is pulled from a paired
//! device.
//!
//! The pull runs in `metal-sync`. The panel reaches it through the door
//! (`GET`/`POST /author/song/pull`, `call/author/playout.rs`), which asks
//! `metal-sync` on loopback. It starts the pull once per song and reads the
//! progress back at most twice a second, so the frame never waits on the
//! network. The lanes panel draws [`line`] under its timeline. Playback stays
//! where it was: on a phone the pulled file is on disk, and the Android audio
//! output is still to come (see `docs/MULTIPLATFORM.md`).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use serde_json::{json, Value};

use super::classes::Theme;
use super::frames;
use super::i18n::{tr, trf};
use crate::call::{Call, Params};

/// How often the progress is read while a pull runs.
const READ_GAP: Duration = Duration::from_millis(500);

#[derive(Default)]
struct Book {
    started: HashSet<String>,
    last: HashMap<String, (Instant, Value)>,
}

fn book() -> &'static Mutex<Book> {
    static BOOK: OnceLock<Mutex<Book>> = OnceLock::new();
    BOOK.get_or_init(Mutex::default)
}

/// The song's status from the door: `has_audio`, `reachable` and the pull.
/// Read at most every [`READ_GAP`]; `None` when the door has no answer.
fn status(door: &dyn Call, song: &str) -> Option<Value> {
    if let Some((at, v)) = book().lock().expect("song pull book").last.get(song) {
        if at.elapsed() < READ_GAP {
            return Some(v.clone());
        }
    }
    let params = Params {
        query: [("song".to_string(), song.to_string())].into_iter().collect(),
        body: Vec::new(),
        tail: None,
    };
    let reply = door.call("GET", "/author/song/pull", params)?;
    if reply.status != 200 {
        return None;
    }
    let v: Value = serde_json::from_slice(&reply.body).ok()?;
    book().lock().expect("song pull book").last.insert(song.to_string(), (Instant::now(), v.clone()));
    Some(v)
}

/// Asks the door to start the pull of `song`, once per session.
fn start(door: &dyn Call, song: &str) {
    if !book().lock().expect("song pull book").started.insert(song.to_string()) {
        return;
    }
    let _ = door.json("POST", "/author/song/pull", Some(json!({ "song": song })));
}

/// The pull's line for the lanes panel: nothing when the song has its audio
/// or no song is open, otherwise where the pull stands.
pub fn line(ui: &mut Ui, theme: &Theme, door: &dyn Call, song: &str) {
    let song = song.trim();
    if song.is_empty() {
        return;
    }
    let Some(status) = status(door, song) else { return };
    if status["has_audio"] == true {
        return;
    }
    start(door, song);
    if status["reachable"] != true {
        let text = tr("sync is not running, so this song has no audio here").to_string();
        ui.label(RichText::new(text).font(theme.font_small.clone()).color(theme.ink_weak));
        return;
    }
    let pull = &status["pull"];
    let (text, running) = match pull["state"].as_str() {
        None | Some("idle" | "searching") => (tr("looking for this song's audio on a paired device…").to_string(), true),
        Some("pulling") => {
            let (got, total) = (pull["got_bytes"].as_u64().unwrap_or(0), pull["total_bytes"].as_u64().unwrap_or(0));
            let pct = if total > 0 { got * 100 / total } else { 0 };
            let from = pull["from"].as_str().unwrap_or("?");
            (trf("pulling the audio from {peer} · {pct}%", &[("peer", &from), ("pct", &pct)]), true)
        }
        Some("missing") => (trf("no paired device has the audio of {song}", &[("song", &song)]), false),
        Some("failed") => {
            let why = pull["error"].as_str().unwrap_or("?").to_string();
            (trf("the audio pull failed: {why}", &[("why", &why)]), false)
        }
        Some(_) => return,
    };
    ui.label(RichText::new(text).font(theme.font_small.clone()).color(theme.ink_weak));
    if running {
        frames::repaint_after(ui.ctx(), READ_GAP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_is_started_once_per_session() {
        let mut b = book().lock().expect("song pull book");
        assert!(b.started.insert("only-once-test".to_string()));
        assert!(!b.started.insert("only-once-test".to_string()));
    }
}
