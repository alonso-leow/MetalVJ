//! P-C: the video I/O routes and the sink pump.
//!
//! `GET /videoio` lists every sink kind (`virtualcam`, `ndi`, `spout`,
//! `syphon`) with whether it works on this machine and, when not, why — plus
//! each screen's live sink status. The write is `POST /outputs {op:"sink",
//! id, kind, target}` (`call::outputs`), the same route the properties page
//! and the `set_output_sink` tool post (rule 6).
//!
//! The pump is one thread: per tick it takes the lock, renders each screen
//! that has a sink through [`super::render::device_frame`] (the picture
//! `/output.jpg` shows — rule 3), lets go, and pushes outside the lock.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// Per screen: the last push's outcome (`error` `None` = ok) and the sink's
/// own status line (`state`: the sender's name, size and frame count).
#[derive(Clone, Default)]
struct SinkStatus {
    error: Option<String>,
    state: String,
}

fn status() -> &'static Mutex<HashMap<String, SinkStatus>> {
    static S: OnceLock<Mutex<HashMap<String, SinkStatus>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Every kind with availability, as JSON rows.
pub(crate) fn modes_json() -> Vec<Value> {
    #[cfg(feature = "desktop")]
    {
        metal_videoio::modes()
            .into_iter()
            .map(|m| {
                json!({
                    "kind": m.kind,
                    "available": m.available,
                    "reason": m.reason,
                    "targets": m.targets.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
                })
            })
            .collect()
    }
    #[cfg(not(feature = "desktop"))]
    {
        ["virtualcam", "ndi", "spout", "syphon"]
            .iter()
            .map(|k| json!({ "kind": k, "available": false, "reason": "this build has no video I/O", "targets": [] }))
            .collect()
    }
}

/// Validates a sink write: an unknown kind or a kind disabled here is
/// refused with its reason (the same sentence `GET /videoio` shows).
pub(crate) fn check_sink(kind: &str, modes: &[Value]) -> Result<(), String> {
    let Some(m) = modes.iter().find(|m| m["kind"] == json!(kind)) else {
        return Err(format!("unknown output kind: {kind}"));
    };
    if m["available"].as_bool() != Some(true) {
        return Err(m["reason"].as_str().unwrap_or("unavailable").to_string());
    }
    Ok(())
}

/// The name a sink publishes under: a Spout or Syphon screen with no name
/// given publishes as the screen's label; the other kinds keep their target.
pub(crate) fn published_as(kind: &str, target: &str, label: &str) -> String {
    if matches!(kind, "spout" | "syphon") && target.trim().is_empty() {
        label.to_string()
    } else {
        target.to_string()
    }
}

/// `GET /videoio`.
pub fn get(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let st = status().lock().map(|g| g.clone()).unwrap_or_default();
    let sinks: Vec<Value> = ctx
        .state
        .outputs
        .list
        .iter()
        .filter_map(|o| {
            let s = o.sink.as_ref()?;
            Some(json!({
                "id": o.id,
                "kind": s.kind,
                "target": s.target,
                "error": st.get(&o.id).and_then(|s| s.error.clone()),
                "state": st.get(&o.id).map(|s| s.state.clone()).unwrap_or_default(),
            }))
        })
        .collect();
    Ok(json!({ "modes": modes_json(), "sinks": sinks }))
}

/// Starts the pump (desktop builds only). ~30 fps, at most 1280 wide.
#[cfg(feature = "desktop")]
pub(crate) fn spawn_pump(shared: std::sync::Arc<Mutex<crate::app::Shared>>) {
    std::thread::Builder::new()
        .name("videoio-pump".into())
        .spawn(move || {
            let mut open: HashMap<String, (crate::output::OutputSink, Box<dyn metal_videoio::Sink>)> = HashMap::new();
            let start = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(33));
                let frames: Vec<(String, crate::output::OutputSink, Vec<u8>, u32, u32)> = {
                    let Ok(mut g) = shared.lock() else { return };
                    let g = &mut *g;
                    let wanted: Vec<crate::output::Output> = g
                        .state
                        .outputs
                        .list
                        .iter()
                        .filter(|o| o.sink.is_some() && o.ceded_to.is_none())
                        .cloned()
                        .collect();
                    if wanted.is_empty() && open.is_empty() {
                        continue;
                    }
                    wanted
                        .into_iter()
                        .filter_map(|o| {
                            let scale = (1280.0 / o.width as f32).min(1.0);
                            let w = ((o.width as f32 * scale) as u32).max(2) & !1;
                            let h = ((o.height as f32 * scale) as u32).max(2);
                            // G3: the frame stays a texture; the sink reads it through
                            // its own non-blocking ring, a frame or two late, and a tick
                            // whose copy is still in flight pushes nothing.
                            let (px, pw, ph) = match super::render::device_texture(g.paint.as_mut(), &mut g.sim, &mut g.state, &o, w, h) {
                                metal_perform::engine::DeviceFrame::Cpu(px, pw, ph) => (px, pw, ph),
                                metal_perform::engine::DeviceFrame::Gpu(f) => g.paint.readback_async(&format!("sink:{}", o.id), &f)?,
                            };
                            // Spout and Syphon publish under the screen's label
                            // when the operator left the name empty.
                            let mut spec = o.sink.clone().expect("filtered");
                            spec.target = published_as(&spec.kind, &spec.target, &o.label);
                            Some((o.id.clone(), spec, px, pw, ph))
                        })
                        .collect()
                };
                let live: Vec<&String> = frames.iter().map(|f| &f.0).collect();
                open.retain(|id, _| live.contains(&id));
                let mut st = status().lock().ok();
                if let Some(st) = st.as_mut() {
                    st.retain(|id, _| live.contains(&id));
                }
                let t = start.elapsed().as_secs_f64();
                for (id, spec, px, w, h) in frames {
                    if open.get(&id).map(|(s, _)| s != &spec).unwrap_or(true) {
                        open.remove(&id);
                        match metal_videoio::open(&spec.kind, &spec.target) {
                            Ok(sink) => {
                                open.insert(id.clone(), (spec.clone(), sink));
                            }
                            Err(e) => {
                                if let Some(st) = st.as_mut() {
                                    st.insert(id, SinkStatus { error: Some(e), state: String::new() });
                                }
                                continue;
                            }
                        }
                    }
                    let (_, sink) = open.get_mut(&id).expect("opened above");
                    let r = sink.push(&px, w, h, t).err();
                    let state = sink.describe();
                    if r.is_some() {
                        open.remove(&id);
                    }
                    if let Some(st) = st.as_mut() {
                        st.insert(id, SinkStatus { error: r, state });
                    }
                }
            }
        })
        .expect("spawn videoio pump");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_kind_is_refused_with_its_reason_and_an_unknown_one_by_name() {
        let modes = vec![
            json!({ "kind": "virtualcam", "available": false, "reason": "the v4l2loopback module is not loaded" }),
            json!({ "kind": "ndi", "available": true, "reason": null }),
        ];
        assert_eq!(check_sink("ndi", &modes), Ok(()));
        assert!(check_sink("virtualcam", &modes).unwrap_err().contains("v4l2loopback"));
        assert!(check_sink("hdmi", &modes).unwrap_err().contains("unknown"));
    }

    #[test]
    fn a_named_screen_publishes_as_its_label_when_the_name_is_empty() {
        assert_eq!(published_as("spout", "", "Screen 1"), "Screen 1");
        assert_eq!(published_as("syphon", "  ", "Screen 1"), "Screen 1");
        assert_eq!(published_as("spout", "Output", "Screen 1"), "Output");
        assert_eq!(published_as("ndi", "", "Screen 1"), "", "NDI keeps an empty target");
    }

    #[test]
    fn every_kind_is_listed_with_a_reason_when_off() {
        let modes = modes_json();
        assert_eq!(modes.len(), 4);
        for m in modes {
            if m["available"] == json!(false) {
                assert!(m["reason"].as_str().is_some_and(|r| !r.is_empty()), "{m}");
            }
        }
    }
}
