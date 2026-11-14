//! Proof frames for one authored Colourist example, headless.
//!
//!     cargo run -p metal-render --bin example-proof -- \
//!         --example data/dataset/colourist/examples/67-p2.json \
//!         --at 1200,1500,1810 --out /tmp/proof
//!
//! An example is sparse keyframes of a model's whole row on the canonical
//! 32 Hz grid (`metal-dash::author::models::example`). This reads one, fills the
//! row between keyframes with `metal_schema::keys`, the per-float ease `metal-dash`'s
//! `dense::interpolate` and the timeline's `interpolate_row` use (hold before
//! the first and after the last keyframe), and draws it through
//! [`metal_render::headless::Renderer::solo`] — the call behind the shell's
//! `/author/render/solo.jpg` preview.
//!
//! It **simulates forward** rather than drawing cold stills: models that build
//! up over time (DLA, sand, terrarium, trails) are empty on a first frame, so
//! the renderer is stepped at `--fps` from `--from` (default: the example's
//! `span` start, else 4 s before the first `--at`) and a JPEG is written each
//! time a requested tick is crossed: `<out>/<id>-t<tick>.jpg`. Model time is
//! the song's own seconds (`tick / 32`), so a proof frame is the frame the
//! song would show at that tick, not at an arbitrary phase.
//!
//! `--video` also writes `<out>/<id>.mp4` of the whole simulated run, through
//! the span's end.

use std::io::Write;
use std::process::{Command, Stdio};

use metal_render::headless::{encode_jpeg, Renderer};

const HZ: f32 = 32.0;

struct Key {
    tick: usize,
    floats: Vec<f32>,
}

fn main() {
    let mut example = None;
    let mut at: Vec<usize> = Vec::new();
    let mut out = String::from(".");
    let (mut w, mut h, mut fps) = (640u32, 360u32, 16.0f32);
    let mut from: Option<usize> = None;
    let mut video = false;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--example" => example = Some(val()),
            "--at" => at = val().split(',').map(|s| s.trim().parse().expect("bad --at tick")).collect(),
            "--out" => out = val(),
            "--w" => w = val().parse().expect("bad --w"),
            "--h" => h = val().parse().expect("bad --h"),
            "--fps" => fps = val().parse().expect("bad --fps"),
            "--from" => from = Some(val().parse().expect("bad --from")),
            "--video" => video = true,
            other => panic!("unknown argument {other}"),
        }
    }
    let path = example.expect("--example <file.json> is required");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let json: serde_json::Value = serde_json::from_str(&text).expect("example is not JSON");
    let model = json["model"].as_str().expect("example has no model").to_string();
    let rate = json["rate"].as_f64().unwrap_or(HZ as f64) as f32;
    assert!((rate - HZ).abs() < 1e-3, "example rate {rate}: author on the canonical 32 Hz grid");
    let mut keys: Vec<Key> = json["ticks"]
        .as_array()
        .expect("example has no ticks")
        .iter()
        .map(|t| Key {
            tick: t["tick"].as_u64().expect("tick") as usize,
            floats: t["floats"].as_array().expect("floats").iter().map(|v| v.as_f64().unwrap() as f32).collect(),
        })
        .collect();
    keys.sort_by_key(|k| k.tick);
    assert!(!keys.is_empty(), "example has no keyframes");
    let width = keys[0].floats.len();
    let tracks = metal_schema::keys::tracks(keys.iter().map(|k| (k.tick, k.floats.as_slice())), width);
    let id = json["part"].as_str().map(str::to_string).unwrap_or_else(|| model.clone());
    let span = json["span"].as_array().and_then(|s| Some((s.first()?.as_u64()? as usize, s.get(1)?.as_u64()? as usize)));
    if at.is_empty() {
        at = keys.iter().map(|k| k.tick).collect();
    }
    at.sort_unstable();

    let mut renderer = Renderer::new();
    let index = renderer
        .ids()
        .iter()
        .position(|m| *m == model)
        .unwrap_or_else(|| panic!("no model {model:?} in the registry"));
    let width = renderer.schemas()[index].width();
    for k in &keys {
        assert_eq!(k.floats.len(), width, "tick {}: {} floats, {model} takes {width}", k.tick, k.floats.len());
    }

    let start = from
        .or(span.map(|s| s.0))
        .unwrap_or_else(|| at[0].saturating_sub((4.0 * HZ) as usize))
        .min(at[0]);
    // A video runs to the span's end; stills stop at the last requested tick.
    let end = match (video, span) {
        (true, Some((_, b))) => (*at.last().unwrap()).max(b),
        _ => *at.last().unwrap(),
    };
    std::fs::create_dir_all(&out).expect("create --out");

    let mut ff = video.then(|| {
        Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pixel_format", "rgba"])
            .args(["-video_size", &format!("{w}x{h}"), "-framerate", &fps.to_string(), "-i", "-"])
            .args(["-an", "-c:v", "libx264", "-pix_fmt", "yuv420p", &format!("{out}/{id}.mp4")])
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn ffmpeg")
    });

    let step = HZ / fps; // ticks per simulated frame
    let mut tick = start as f32;
    let mut pending = at.iter().copied().peekable();
    while tick <= end as f32 + 1e-3 {
        // Land exactly on a requested tick when the next step would pass it.
        let next_req = pending.peek().copied();
        let t = match next_req {
            Some(r) if (r as f32) < tick + step && (r as f32) >= tick => r as f32,
            _ => tick,
        };
        let row = metal_schema::keys::row_at(&tracks, t).expect("keyed rows");
        let (rgba, rw, rh) = renderer.solo(index, &row, t / HZ, w, h, false);
        if let Some(child) = ff.as_mut() {
            child.stdin.as_mut().unwrap().write_all(&rgba).expect("ffmpeg write");
        }
        if next_req == Some(t as usize) && (t - t.floor()).abs() < 1e-6 {
            let file = format!("{out}/{id}-t{}.jpg", t as usize);
            std::fs::write(&file, encode_jpeg(&rgba, rw, rh)).expect("write jpg");
            println!("{file}  ({:.2}s)", t / HZ);
            pending.next();
            tick = t + step;
        } else {
            tick += step;
        }
    }
    if let Some(mut child) = ff {
        drop(child.stdin.take());
        child.wait().expect("ffmpeg");
        println!("{out}/{id}.mp4");
    }
}
