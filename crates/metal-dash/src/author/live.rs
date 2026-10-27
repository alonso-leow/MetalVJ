//! The `/live` receiver: the row `fw-corpus` is showing, arriving
//! here while you drag its sliders.
//!
//! The contract is `docs/SUITE_STUDIO.md`'s, written before anything
//! implemented it: the corpus server relays `{"model": "<schema id>",
//! "floats": [...]}` as a `POST /live` to a port it does not expect anyone to
//! be listening on, and swallows the connection error when nobody is. So the
//! shape here is fixed by that document, not chosen: a plain HTTP POST on its
//! own port, and no response the pusher reads.

use std::sync::Mutex;
#[cfg(test)]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(test)]
use std::net::{TcpListener, TcpStream};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use serde_json::Value;


/// The last row pushed in, if one has arrived since the render loop looked.
///
/// Latest-wins rather than a queue on purpose: a drag produces pushes far
/// faster than frames, and every one except the newest describes a slider
/// position already in the past. Queueing them would make the preview fall
/// further behind the longer you drag.
#[derive(Default)]
pub struct Live {
    latest: Mutex<Option<(String, Vec<f32>)>>,
}

impl Live {
    #[cfg(test)]
    pub fn set(&self, model: String, floats: Vec<f32>) {
        *self.latest.lock().expect("live lock") = Some((model, floats));
    }

    pub fn take(&self) -> Option<(String, Vec<f32>)> {
        self.latest.lock().expect("live lock").take()
    }
}

/// What a push turns out to be once it is checked against this build.
#[derive(Debug, PartialEq)]
pub enum Adopt {
    /// Index into the renderer's model list.
    Row(usize),
    /// A model id this build does not have.
    Unknown,
    /// The right model, the wrong row width — the two sides are working off
    /// different copies of `data/schema.json`.
    WrongWidth { expected: usize, got: usize },
}

/// Checks a push against this build's models without touching any state, so
/// the decision can be tested on its own.
///
/// Both failures are reported rather than ignored: a silent no-op here looks
/// exactly like "the renderer is not running", and the whole point of the
/// bridge is that you can tell those apart.
pub fn resolve(id: &str, got: usize, ids: &[&'static str], widths: &[usize]) -> Adopt {
    let Some(index) = ids.iter().position(|k| *k == id) else {
        return Adopt::Unknown;
    };
    let expected = widths[index];
    if got != expected {
        return Adopt::WrongWidth { expected, got };
    }
    Adopt::Row(index)
}

#[cfg(test)]
/// Binds the live port and feeds `live` from it.
///
/// A bind failure is reported and shrugged off: the preview is useful on its
/// own, and the corpus studio is explicitly built to treat a missing receiver
/// as a non-event. Returns whether it is listening, which is only used to say
/// so at startup.
pub fn spawn(port: u16, live: Arc<Live>) -> bool {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("  no /live receiver: cannot bind 127.0.0.1:{port}: {e}");
            return false;
        }
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some((model, floats)) = read_push(stream) {
                live.set(model, floats);
            }
        }
    });
    true
}

#[cfg(test)]
/// Reads one `POST /live` and answers it. Only `Content-Length` matters; the
/// pusher reads no response, so a bare 200 is the whole reply.
pub fn read_push(stream: TcpStream) -> Option<(String, Vec<f32>)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;

    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = header.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;

    let mut stream = stream;
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");

    let value: Value = serde_json::from_slice(&body).ok()?;
    let model = value.get("model")?.as_str()?.to_string();
    let floats = value
        .get("floats")?
        .as_array()?
        .iter()
        .map(|v| v.as_f64().unwrap_or(0.0) as f32)
        .collect();
    Some((model, floats))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: [&str; 3] = ["geoterrain", "oracle", "turing"];
    const WIDTHS: [usize; 3] = [18, 50, 48];

    #[test]
    fn a_known_model_at_the_right_width_resolves_to_its_index() {
        assert_eq!(resolve("oracle", 50, &IDS, &WIDTHS), Adopt::Row(1));
        assert_eq!(resolve("turing", 48, &IDS, &WIDTHS), Adopt::Row(2));
    }

    #[test]
    fn an_unknown_id_and_a_stale_width_are_told_apart() {
        assert_eq!(resolve("retired", 26, &IDS, &WIDTHS), Adopt::Unknown);
        // The exact case the manifest reconciliation fixed: the corpus side
        // holding a 31-float chamber against a 26-float renderer.
        assert_eq!(
            resolve("oracle", 31, &IDS, &WIDTHS),
            Adopt::WrongWidth {
                expected: 50,
                got: 31
            }
        );
    }

    /// Drives the real socket path, with a request written exactly the way
    /// `fw-corpus::push_live` writes it — that function is the only
    /// client this will ever have, so matching it byte for byte is the test.
    #[test]
    fn a_push_over_a_real_socket_arrives_intact() {
        let live = Arc::new(Live::default());
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        let port = listener.local_addr().expect("addr").port();
        let mine = Arc::clone(&live);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some((m, f)) = read_push(stream) {
                    mine.set(m, f);
                    return;
                }
            }
        });

        let body = br#"{"model":"turing","floats":[0.5,-1.25,3.0]}"#;
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        write!(
            stream,
            "POST /live HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("head");
        stream.write_all(body).expect("body");

        let mut got = None;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            got = live.take();
            if got.is_some() {
                break;
            }
        }
        let (model, floats) = got.expect("the push never arrived");
        assert_eq!(model, "turing");
        assert_eq!(floats, vec![0.5, -1.25, 3.0]);
    }

    #[test]
    fn taking_twice_yields_nothing_the_second_time() {
        let live = Live::default();
        live.set("oracle".into(), vec![1.0]);
        assert!(live.take().is_some());
        assert!(
            live.take().is_none(),
            "a row must be adopted once, not every frame"
        );
    }

    #[test]
    fn a_later_push_replaces_an_unread_one() {
        let live = Live::default();
        live.set("oracle".into(), vec![1.0]);
        live.set("oracle".into(), vec![2.0]);
        assert_eq!(
            live.take().expect("latest").1,
            vec![2.0],
            "latest wins, no backlog"
        );
    }
}
