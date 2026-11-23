//! A short, silent demo clip per model, headless — for the song gate and for
//! anyone who wants to see what an animation looks like before gating it.
//!
//!     cargo run -p metal-render --bin model-demo -- --out tools/song-gate/demos
//!
//! For every model in the registry it draws `seconds * fps` frames through
//! [`metal_render::headless::Renderer::solo`] and pipes the raw RGBA straight
//! into ffmpeg, which encodes an mp4. A mid-clip frame is also written as a
//! JPEG poster beside it.
//!
//! **A transmission** (`wake`) is pointed at the sample reel under
//! `data/reels/` (default `example-footage`, override with `--footage`) so it
//! previews its effect on footage rather than the "no footage" slate it draws
//! with no library.
//!
//! **By default the floats are the schema's own defaults** — the model's
//! declared resting state, its own colours, no effects — so a clip shows what
//! the animation is. `--vary` opts into a *diverse* row instead (expressive
//! floats pushed to a deterministic point in their range, colour slots painted
//! from a handful of palettes); useful for stressing a model, misleading as a
//! catalogue, so it is off by default.
//!
//! `--only <id>` renders one model; `--w/--h/--fps/--seconds` change the clip.
//! The output is deterministic: same repo, same clips.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use metal_reel::curate::{Cue, Play};
use metal_reel::Library;
use metal_render::headless::{encode_jpeg, Renderer};
use metal_schema::{Block, FloatSpec};

/// Four stops x RGB, the shape `Block::ColorSlot` carries. Six palettes that
/// span the families the plan names (warm/tropical, night/neon, earthy,
/// pastel, mono, club) so no two neighbouring models open in the same colours.
const PALETTES: [[f32; 12]; 6] = [
    // warm / tropical
    [1.00, 0.55, 0.10, 1.00, 0.25, 0.20, 0.95, 0.10, 0.45, 1.00, 0.80, 0.30],
    // night / neon
    [0.10, 0.05, 0.35, 0.55, 0.05, 0.85, 0.05, 0.85, 0.95, 0.85, 0.20, 1.00],
    // earthy / salsa
    [0.55, 0.35, 0.12, 0.80, 0.45, 0.10, 0.25, 0.45, 0.20, 0.95, 0.75, 0.25],
    // pastel / romantic
    [0.98, 0.75, 0.80, 0.75, 0.85, 0.98, 0.80, 0.95, 0.85, 0.95, 0.90, 0.70],
    // mono / industrial
    [0.05, 0.05, 0.06, 0.30, 0.32, 0.34, 0.65, 0.67, 0.70, 0.98, 0.98, 0.98],
    // high-contrast club
    [0.90, 0.05, 0.25, 0.05, 0.90, 0.60, 0.10, 0.25, 0.95, 1.00, 1.00, 0.10],
];

struct Args {
    out: String,
    w: u32,
    h: u32,
    fps: u32,
    seconds: f32,
    only: Option<String>,
    vary: bool,
    footage: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        out: "tools/song-gate/demos".into(),
        w: 512,
        h: 288,
        fps: 12,
        seconds: 4.0,
        only: None,
        vary: false,
        footage: "example-footage".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out" => a.out = it.next().expect("--out needs a path"),
            "--w" => a.w = it.next().expect("--w needs a number").parse().expect("bad --w"),
            "--h" => a.h = it.next().expect("--h needs a number").parse().expect("bad --h"),
            "--fps" => a.fps = it.next().expect("--fps needs a number").parse().expect("bad --fps"),
            "--seconds" => {
                a.seconds = it.next().expect("--seconds needs a number").parse().expect("bad --seconds")
            }
            "--only" => a.only = Some(it.next().expect("--only needs an id")),
            "--vary" => a.vary = true,
            "--footage" => a.footage = it.next().expect("--footage needs a library name"),
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

/// One float chosen to show the parameter, not to sit at its default.
///
/// `drive` is the declared freedom: a structural float (drive < 0.4 — a regime
/// selector like Turing's feed/kill) is left exactly where the model put it,
/// because moving it does not make the model more dramatic, it makes it a
/// different and usually blank one. Everything else lands somewhere in its
/// legal range, at a point that varies by model and by position so the row is
/// diverse but the same on every run.
fn pick(s: &FloatSpec, model: usize, field: usize) -> f32 {
    if s.drive < 0.4 || s.max <= s.min {
        return s.default;
    }
    let phase = model as f32 * 1.7 + field as f32 * 0.9;
    let frac = 0.5 + 0.45 * phase.sin();
    s.min + (s.max - s.min) * frac
}

/// The row this model is demoed with: schema width, expressive floats varied,
/// colour slots painted, camera left where the model asked to be seen from.
fn demo_floats(schema: &metal_schema::Schema, model: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(schema.width());
    let mut slot = 0usize;
    for block in schema.blocks {
        match block {
            Block::Floats(fs) => {
                for (i, s) in fs.iter().enumerate() {
                    out.push(pick(s, model, i));
                }
            }
            Block::ColorSlot(_, _) => {
                let p = PALETTES[(model + slot) % PALETTES.len()];
                out.extend(p);
                slot += 1;
            }
            Block::Camera(view) => out.extend(view),
        }
    }
    out
}

/// Points a transmission at the sample reel under `data/reels/<name>` and cues
/// its first passage from t=0, so the clip previews the model's effect on real
/// footage instead of the slate it draws when it has none. A missing library is
/// reported and left as the slate — a demo must never fail for lack of footage.
fn attach_footage(renderer: &mut Renderer, index: usize, name: &str) {
    if name.is_empty() {
        return;
    }
    let lib = match Library::load(Path::new("data"), name) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("footage {name}: {e} (rendering the slate)");
            return;
        }
    };
    let cue = lib
        .clips
        .first()
        .and_then(|c| c.passages.first().map(|p| (c, p)))
        .map(|(c, p)| Cue {
            clip: 0,
            clip_id: c.id.clone(),
            passage: 0,
            from: p.from as f32 / lib.rate.max(1e-6),
            span: p.seconds,
            since: 0.0,
            confidence: 1.0,
            because: None,
            play: Play::default(),
            authored: None,
        });
    if let Some(reel) = renderer.reel(index) {
        reel.attach(Some(Arc::new(lib)));
        reel.cue(cue);
    }
}

fn ffmpeg(w: u32, h: u32, fps: u32, path: &str) -> std::process::Child {
    Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &format!("{w}x{h}"),
            "-framerate",
            &fps.to_string(),
            "-i",
            "-",
            "-an",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            path,
        ])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn ffmpeg (is it on PATH?)")
}

fn main() {
    let a = parse_args();
    std::fs::create_dir_all(&a.out).expect("create output dir");

    let mut renderer = Renderer::new();
    let ids: Vec<&'static str> = renderer.ids();
    let schemas: Vec<&'static metal_schema::Schema> = renderer.schemas();
    let footage: Vec<bool> = renderer.footage();
    let frames = (a.seconds * a.fps as f32).round().max(1.0) as u32;

    let mut done = 0usize;
    for (i, id) in ids.iter().enumerate() {
        // A tombstone (a retired model's row) has an empty schema; it draws
        // nothing, so it gets no preview and no entry in the gate's list.
        if schemas[i].width() == 0 {
            continue;
        }
        if let Some(only) = &a.only {
            if only != id {
                continue;
            }
        }
        let floats = if a.vary {
            demo_floats(schemas[i], i)
        } else {
            schemas[i].defaults()
        };
        assert_eq!(floats.len(), schemas[i].width(), "{id}: row width disagrees with schema");

        // A transmission draws its own "no footage" slate without a library.
        // Attach the sample reel so the clip shows the effect, not the slate.
        if footage[i] {
            attach_footage(&mut renderer, i, &a.footage);
        }

        let path = format!("{}/{id}.mp4", a.out);
        let mut child = ffmpeg(a.w, a.h, a.fps, &path);
        let mut stdin = child.stdin.take().expect("ffmpeg stdin");
        let mut poster: Option<Vec<u8>> = None;
        // Poster from a mid-clip frame, not frame 0: a transmission has not
        // decoded its footage yet at t=0 (it was black), and a model that fills
        // over time (DLA, sand, terrarium) is empty at the start. The middle of
        // the clip is the honest still.
        let poster_at = (frames as f32 * 0.6).round() as u32;
        for f in 0..frames {
            let t = f as f32 / a.fps as f32;
            let (rgba, w, h) = renderer.solo(i, &floats, t, a.w, a.h, false);
            if f == poster_at {
                poster = Some(encode_jpeg(&rgba, w, h));
            }
            stdin.write_all(&rgba).expect("write frame to ffmpeg");
        }
        drop(stdin);
        let status = child.wait().expect("wait for ffmpeg");
        assert!(status.success(), "ffmpeg failed for {id}");
        if let Some(jpg) = poster {
            std::fs::write(format!("{}/{id}.jpg", a.out), jpg).expect("write poster");
        }
        done += 1;
        println!("[{done}] {id}: {frames} frames, {}x{} -> {path}", a.w, a.h);
    }
    println!("wrote {done} demo(s) to {}", a.out);
}
