//! The venue-generation experiment: camera images in, a venue TOML out,
//! scored by `metal_perform::venue::Venue::check`. The same request goes to the
//! provider's big LLM (the default) or to a local model, so the two can be
//! compared on the same images with the same numbers.
//!
//! ```text
//! cargo run -p metal-dash --example venue_gen -- IMAGE... [options]
//! cargo run -p metal-dash --example venue_gen -- --camera [options]
//!
//!   --camera [DEVICE]          grab the image from a camera instead of a file
//!                              (the first one that answers, or DEVICE, e.g.
//!                              /dev/video0) — the same capture the harness's
//!                              `capture_camera_frame` makes; saved beside
//!                              --out as `<out>.camera.png`
//!   --backend provider|local   provider (default): the harness's configured model
//!   --url URL                  local server, default http://127.0.0.1:8080/v1/chat/completions
//!   --model NAME               the local model's name (required with --backend local)
//!   --runs N                   answers to ask for, default 1
//!   --temperature T            sampling temperature, default: the backend's
//!   --max-tokens N             cap on the answer's length, default: the backend's
//!   --ask TEXT                 the operator's words about the place
//!   --data DIR                 data root, default the repo's data/
//!   --out FILE                 write the best answer here, and each image as
//!                              the model saw it (under the grid) beside it,
//!                              as `<out>.grid-<k>.png`
//!   --log FILE                 append one JSON line per run
//! ```
//!
//! Every image goes out under `locate`'s grid, and the venue's image points
//! come back as its cells (`metal_agent::venue_gen`, `metal_perform::venue::GRID`).
//! Each run prints one line: seconds, whether it parsed, the check's errors
//! and warnings, and the three worst-case numbers (corner reprojection,
//! projector distance, projector aim). See the design notes, "3D venues", for the
//! protocol these runs feed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use metal_agent::venue_gen::{self, Backend, Image};
use metal_perform::venue::Venue;
use serde_json::json;

// The grid drawn over the images and the grid the cells are read back on
// are one grid: this is the one place both crates meet.
const _: () = assert!(
    metal_perform::venue::GRID == venue_gen::GRID,
    "metal_perform::venue::GRID must be the grid venue_gen draws (locate's COARSE_GRID)"
);

struct Args {
    images: Vec<PathBuf>,
    /// `Some(None)`: any camera; `Some(Some(dev))`: that one.
    camera: Option<Option<String>>,
    backend: Backend,
    runs: usize,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    ask: String,
    data: PathBuf,
    out: Option<PathBuf>,
    log: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut images = Vec::new();
    let (mut backend, mut url, mut model) = ("provider".to_string(), venue_gen::LOCAL_URL.to_string(), None);
    let mut a = Args {
        images: Vec::new(),
        camera: None,
        backend: Backend::Provider,
        runs: 1,
        temperature: None,
        max_tokens: None,
        ask: String::new(),
        data: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"),
        out: None,
        log: None,
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(arg) = it.next() {
        if arg == "--camera" {
            let device = it.next_if(|v| v.starts_with("/dev/"));
            a.camera = Some(device);
            continue;
        }
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--backend" => backend = value()?,
            "--url" => url = value()?,
            "--model" => model = Some(value()?),
            "--runs" => a.runs = value()?.parse().map_err(|_| "--runs takes a number")?,
            "--temperature" => a.temperature = Some(value()?.parse().map_err(|_| "--temperature takes a number")?),
            "--max-tokens" => a.max_tokens = Some(value()?.parse().map_err(|_| "--max-tokens takes a number")?),
            "--ask" => a.ask = value()?,
            "--data" => a.data = value()?.into(),
            "--out" => a.out = Some(value()?.into()),
            "--log" => a.log = Some(value()?.into()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path => images.push(PathBuf::from(path)),
        }
    }
    let total = images.len() + usize::from(a.camera.is_some());
    if total == 0 || total > 3 {
        return Err("give one to three images, one per camera (`--camera` counts as one)".into());
    }
    a.images = images;
    a.backend = match backend.as_str() {
        "provider" => Backend::Provider,
        "local" => Backend::Local {
            url,
            model: model.ok_or("--backend local needs --model")?,
        },
        other => return Err(format!("--backend is provider or local, not {other}")),
    };
    Ok(a)
}

fn main() {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("venue_gen: {e}\n(see the doc comment at the top of crates/metal-dash/examples/venue_gen.rs)");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&a) {
        eprintln!("venue_gen: {e}");
        std::process::exit(1);
    }
}

fn run(a: &Args) -> Result<(), String> {
    let agent = a.data.join("agent");
    let prompt = std::fs::read_to_string(agent.join("venue_prompt.md")).map_err(|e| format!("venue_prompt.md: {e}"))?;
    let examples = venue_gen::examples(&agent.join("venues"))?;
    let mut images = a.images.iter().map(|p| Image::read(p)).collect::<Result<Vec<_>, _>>()?;
    if let Some(device) = &a.camera {
        let (device, png) = match device {
            Some(d) => (
                d.clone(),
                metal_ingest::camera::capture_frame_png(Path::new(d), 1280, 720).map_err(|e| e.to_string())?,
            ),
            None => metal_ingest::camera::capture_any_png(1280, 720).map_err(|errors| {
                let why: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
                if why.is_empty() { "no camera present".to_string() } else { why.join("; ") }
            })?,
        };
        println!("camera: one frame from {device}");
        if let Some(out) = &a.out {
            let saved = out.with_extension("camera.png");
            std::fs::write(&saved, &png).map_err(|e| format!("{}: {e}", saved.display()))?;
            println!("camera frame saved to {}", saved.display());
        }
        images.push(Image { name: "camera.png".into(), mime: "image/png".into(), bytes: png });
    }
    let provider = metal_agent::config::ProviderConfig::load(a.data.join("ai.json")).map_err(|e| e.to_string())?;
    let config = venue_gen::config_for(&a.backend, provider)?;
    let label = match &a.backend {
        Backend::Provider => "provider",
        Backend::Local { .. } => "local",
    };
    let mut body = venue_gen::request(&config.model, &prompt, &examples, &images, &a.ask, a.temperature)?;
    if let Some(n) = a.max_tokens {
        body["max_tokens"] = json!(n);
    }
    if let Some(out) = &a.out {
        for (k, png) in venue_gen::sent_images(&body).iter().enumerate() {
            let saved = out.with_extension(format!("grid-{}.png", k + 1));
            std::fs::write(&saved, png).map_err(|e| format!("{}: {e}", saved.display()))?;
            println!("image {} as the model sees it: {}", k + 1, saved.display());
        }
    }
    println!(
        "{} image(s) under a {g}x{g} grid, {} example(s), {label} {} at {}",
        images.len(),
        examples.len(),
        config.model,
        config.url,
        g = venue_gen::GRID,
    );

    // (errors, warnings, reprojection) — lower is better — and the text.
    let mut best: Option<((usize, usize, f32), String)> = None;
    for run in 1..=a.runs {
        let started = Instant::now();
        let answer = venue_gen::generate(&config, &body);
        let seconds = started.elapsed().as_secs_f32();
        let mut row = json!({
            "backend": label,
            "model": config.model,
            "run": run,
            "seconds": seconds,
            "images": images.iter().map(|i| &i.name).collect::<Vec<_>>(),
            "temperature": a.temperature,
            "grid": venue_gen::GRID,
        });
        if let Ok(answer) = &answer {
            row["usage"] = answer.usage.clone();
            row["answer"] = json!(answer.text);
        }
        match answer.and_then(|answer| Venue::parse(&answer.text).map(|v| (v, answer.text))) {
            Ok((v, text)) => {
                let r = v.check();
                println!(
                    "run {run}  {seconds:6.1}s  parsed  errors {}  warnings {}  reprojection {}  distance {}  angle {}",
                    r.errors(),
                    r.warnings(),
                    fmt(r.reprojection),
                    fmt(r.distance),
                    fmt(r.angle)
                );
                for p in r.problems.iter().take(6) {
                    println!("        {:?} {}: {}", p.level, p.at, p.what);
                }
                row["parsed"] = json!(true);
                row["errors"] = json!(r.errors());
                row["warnings"] = json!(r.warnings());
                row["reprojection"] = json!(r.reprojection);
                row["distance"] = json!(r.distance);
                row["angle"] = json!(r.angle);
                row["shots"] = json!(v.shots.len());
                row["masks"] = json!(v.masks.len());
                row["problems"] = json!(r.problems);
                let rank = (r.errors(), r.warnings(), r.reprojection.unwrap_or(f32::INFINITY));
                if best.as_ref().is_none_or(|(b, _)| rank < *b) {
                    best = Some((rank, text));
                }
            }
            Err(e) => {
                println!("run {run}  {seconds:6.1}s  failed: {}", e.lines().next().unwrap_or_default());
                row["parsed"] = json!(false);
                row["error"] = json!(e);
            }
        }
        if let Some(log) = &a.log {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .map_err(|e| format!("{}: {e}", log.display()))?;
            writeln!(f, "{row}").map_err(|e| e.to_string())?;
        }
    }
    if let (Some(out), Some((_, text))) = (&a.out, &best) {
        std::fs::write(out, text).map_err(|e| format!("{}: {e}", out.display()))?;
        println!("best answer written to {}", out.display());
    }
    Ok(())
}

fn fmt(v: Option<f32>) -> String {
    v.map_or("-".into(), |x| format!("{x:.3}"))
}

#[cfg(test)]
mod tests {
    use metal_agent::locate::strips_to_region;
    use metal_perform::venue::{cell_point, point_cell, GRID};

    /// A cell the venue reads back is the strip `locate` resolves: its
    /// centre part is the centre of `strips_to_region`'s region, and its
    /// corner parts stay inside it.
    #[test]
    fn a_venue_cell_is_a_locate_strip() {
        for c in 1..=GRID {
            for r in 1..=GRID {
                let region = strips_to_region(GRID, &[c], &[r]).unwrap();
                let (cx, cy) = region.center();
                let centre = cell_point(c as i32, r as i32, 5).unwrap();
                assert!((centre[0] as f64 - cx).abs() < 1e-6 && (centre[1] as f64 - cy).abs() < 1e-6);
                for part in 1..=9u8 {
                    let p = cell_point(c as i32, r as i32, part).unwrap();
                    assert!(region.x0 < p[0] as f64 && (p[0] as f64) < region.x1);
                    assert!(region.y0 < p[1] as f64 && (p[1] as f64) < region.y1);
                    assert_eq!(point_cell(p), [c as i32, r as i32, part as i32]);
                }
            }
        }
    }
}
