//! What the HTTP wrapper does around the picture routes (`/preview.jpg`,
//! `/source.jpg`, `/output.jpg`, `/node.jpg`, `/frame?device=`): which codec a
//! caller gets, and a one-frame cache of the encoded answer.
//!
//! Serving a frame costs a GPU readback plus an encode (~1% of a core per fps
//! at 720p, `docs/PERF-2026-10-29.md` §5). A caller on this machine takes QOI
//! or raw RGBA instead of JPEG (a faster encode and decode, lossless), and
//! two askers inside one frame time share one render and one encode.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long an encoded frame answers the same request again: one 60 fps
/// frame. A newer ask re-renders.
pub const TTL: Duration = Duration::from_micros(16_667);

/// Entries kept at once; a request mix wider than this just evicts.
const CAP: usize = 16;

/// The routes that answer a picture through `render::image`.
const IMAGE_ROUTES: [&str; 5] = ["/preview.jpg", "/source.jpg", "/output.jpg", "/node.jpg", "/frame"];

/// Whether `path` + `query` is a picture route this module negotiates and
/// caches. `/frame?layer=` is the peer layer route, which carries its own
/// codec choice (`remote_layers::choose_fmt`) and a frame number, so it is not.
pub fn is_image_route(path: &str, query: &HashMap<String, String>) -> bool {
    IMAGE_ROUTES.contains(&path) && !(path == "/frame" && query.contains_key("layer"))
}

/// The codec a caller gets: `Some("qoi" | "rgba")` for a loopback caller that
/// asked for it with `fmt=` or an `Accept: image/qoi` header, `None` (JPEG, or
/// whatever else `fmt` says) for everyone else. Off-machine callers always get
/// JPEG: the wire there is the cost, and a browser cannot show QOI.
pub fn local_format(loopback: bool, fmt: Option<&str>, accept: Option<&str>) -> Option<&'static str> {
    if !loopback {
        return None;
    }
    match fmt {
        Some("qoi") => Some("qoi"),
        Some("rgba") => Some("rgba"),
        Some(_) => None,
        None => accept.filter(|a| a.contains("image/qoi")).map(|_| "qoi"),
    }
}

/// Settles `fmt` in `query` for one picture request (see [`local_format`]).
pub fn negotiate(query: &mut HashMap<String, String>, loopback: bool, accept: Option<&str>) {
    match local_format(loopback, query.get("fmt").map(String::as_str), accept) {
        Some(fmt) => {
            query.insert("fmt".into(), fmt.into());
        }
        None => {
            if matches!(query.get("fmt").map(String::as_str), Some("qoi" | "rgba")) {
                query.remove("fmt");
            }
        }
    }
}

/// The cache key: route plus the whole query, sorted, so the graph, model,
/// device, size and codec all take part.
pub fn key(path: &str, query: &HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = query.iter().collect();
    pairs.sort();
    let mut key = path.to_string();
    for (k, v) in pairs {
        key.push('&');
        key.push_str(k);
        key.push('=');
        key.push_str(v);
    }
    key
}

struct Entry {
    at: Instant,
    kind: &'static str,
    body: Vec<u8>,
}

#[derive(Default)]
pub struct Cache {
    entries: HashMap<String, Entry>,
}

impl Cache {
    /// The encoded answer for `key` if one was stored less than [`TTL`] ago.
    pub fn get(&self, key: &str, now: Instant) -> Option<(&'static str, Vec<u8>)> {
        let e = self.entries.get(key)?;
        (now.saturating_duration_since(e.at) < TTL).then(|| (e.kind, e.body.clone()))
    }

    pub fn put(&mut self, key: String, kind: &'static str, body: Vec<u8>, now: Instant) {
        self.entries.retain(|_, e| now.saturating_duration_since(e.at) < TTL);
        if self.entries.len() >= CAP {
            self.entries.clear();
        }
        self.entries.insert(key, Entry { at: now, kind, body });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_key_ignores_query_order_and_tells_size_graph_and_codec_apart() {
        let a = key("/preview.jpg", &q(&[("w", "640"), ("h", "360")]));
        assert_eq!(a, key("/preview.jpg", &q(&[("h", "360"), ("w", "640")])));
        assert_ne!(a, key("/preview.jpg", &q(&[("w", "320"), ("h", "360")])));
        assert_ne!(a, key("/output.jpg", &q(&[("w", "640"), ("h", "360")])));
        assert_ne!(a, key("/preview.jpg", &q(&[("w", "640"), ("h", "360"), ("fmt", "qoi")])));
        assert_ne!(
            key("/node.jpg", &q(&[("graph", "a")])),
            key("/node.jpg", &q(&[("graph", "b")]))
        );
    }

    #[test]
    fn a_stored_frame_answers_within_a_frame_time_and_expires_after() {
        let mut cache = Cache::default();
        let t0 = Instant::now();
        cache.put("k".into(), "image/jpeg", vec![1, 2, 3], t0);
        assert_eq!(cache.get("k", t0 + TTL / 2), Some(("image/jpeg", vec![1, 2, 3])));
        assert_eq!(cache.get("k", t0 + TTL), None, "a new frame time re-renders");
        assert_eq!(cache.get("other", t0), None);
    }

    #[test]
    fn the_cache_stays_bounded() {
        let mut cache = Cache::default();
        let t0 = Instant::now();
        for i in 0..(CAP * 3) {
            cache.put(format!("k{i}"), "image/jpeg", vec![0], t0);
        }
        assert!(cache.entries.len() <= CAP);
    }

    #[test]
    fn only_loopback_gets_the_local_codec() {
        assert_eq!(local_format(true, Some("qoi"), None), Some("qoi"));
        assert_eq!(local_format(true, Some("rgba"), None), Some("rgba"));
        assert_eq!(local_format(true, None, Some("image/qoi, */*")), Some("qoi"));
        assert_eq!(local_format(true, None, Some("image/jpeg")), None);
        assert_eq!(local_format(true, Some("jpeg"), Some("image/qoi")), None, "an explicit fmt wins");
        assert_eq!(local_format(false, Some("qoi"), Some("image/qoi")), None);
    }

    #[test]
    fn negotiate_turns_a_remote_qoi_ask_into_jpeg() {
        let mut remote = q(&[("fmt", "qoi"), ("w", "64")]);
        negotiate(&mut remote, false, None);
        assert!(!remote.contains_key("fmt"));
        let mut local = q(&[("w", "64")]);
        negotiate(&mut local, true, Some("image/qoi"));
        assert_eq!(local.get("fmt").map(String::as_str), Some("qoi"));
    }

    #[test]
    fn the_peer_layer_route_is_not_a_cached_picture() {
        assert!(is_image_route("/frame", &q(&[("device", "a")])));
        assert!(!is_image_route("/frame", &q(&[("layer", "l1")])));
        assert!(!is_image_route("/state", &q(&[])));
    }
}
