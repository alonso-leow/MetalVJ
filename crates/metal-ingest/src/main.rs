//! Turns two camera angles into a shot bundle the editor can annotate.
//!
//! Runs on the VPS, where the videos are uploaded and the CPU is not needed
//! for anything interactive. The bundle comes home over rsync.

mod sync;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use metal_schema::platform::Command;

use clap::{Args, Parser, Subcommand};
use metal_ingest::video;
use metal_shot::{Camera, Manifest, Pass, Shot, Sync};

#[derive(Parser)]
#[command(name = "metal-ingest", about = "Two camera angles in, one shot bundle out")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build a shot from two videos of the same take.
    New(New),
    /// Add a denser pass over one segment of an existing shot.
    Window(Window),
    /// Print what a shot contains.
    Info { shot: PathBuf },
    /// Compress a shot for the trip home.
    Pack {
        shot: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Args)]
struct New {
    /// Video from the first angle. Its clock becomes the shot clock.
    #[arg(short = 'a', long)]
    cam_a: PathBuf,
    /// Video from the second angle.
    #[arg(short = 'b', long)]
    cam_b: PathBuf,
    #[arg(long)]
    name: String,
    /// Where the bundle goes; `<out>/<name>.shot`.
    #[arg(long, default_value = ".")]
    out: PathBuf,
    /// Frames per second to extract. Defaults to the rate the notation is
    /// keyed on, so every key of a survey pass has a frame under it; taken
    /// from `metal_rig::motion::KEY_RATE` rather than repeated here, because
    /// two copies of one number drift the first time only one is edited.
    /// Dense keying happens in a window over the segment being annotated.
    #[arg(long, default_value_t = metal_rig::motion::KEY_RATE)]
    rate: f64,
    /// Skip audio alignment and use this offset: how far B lags A, seconds.
    #[arg(long)]
    offset: Option<f64>,
    /// Widest offset the alignment will consider, seconds.
    #[arg(long, default_value_t = 15.0)]
    max_offset: f64,
    /// Downscale frames to this width. Full resolution is the default because
    /// phalanges are only a few pixels across as it is.
    #[arg(long)]
    scale: Option<u32>,
    /// JPEG quality, 2 (best) to 31.
    #[arg(long, default_value_t = 3)]
    quality: u8,
    /// Trim the shot clock to this range, seconds.
    #[arg(long)]
    start: Option<f64>,
    #[arg(long)]
    end: Option<f64>,
    /// Recording the score was transcribed against, to put notes on the same
    /// clock as the frames.
    #[arg(long)]
    score_audio: Option<PathBuf>,
    /// Extract even if the estimate says the disk will not take it.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct Window {
    shot: PathBuf,
    #[arg(long)]
    id: String,
    /// Shot-clock seconds.
    #[arg(long)]
    start: f64,
    #[arg(long)]
    end: f64,
    #[arg(long, default_value_t = 30.0)]
    rate: f64,
    #[arg(long)]
    scale: Option<u32>,
    #[arg(long, default_value_t = 2)]
    quality: u8,
    #[arg(long)]
    force: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Cmd::New(args) => new(args),
        Cmd::Window(args) => window(args),
        Cmd::Info { shot } => info(&shot),
        Cmd::Pack { shot, out } => pack(&shot, out),
    }
}

fn new(args: New) -> Result<(), Box<dyn std::error::Error>> {
    let a = video::probe(&args.cam_a)?;
    let b = video::probe(&args.cam_b)?;
    println!(
        "a  {}x{} @ {:.3} fps, {:.1}s\nb  {}x{} @ {:.3} fps, {:.1}s",
        a.width, a.height, a.fps, a.duration, b.width, b.height, b.fps, b.duration
    );

    let (offset, method, confidence, ambiguity) = match args.offset {
        Some(o) => (o, "manual", 1.0, 0.0),
        None => {
            println!("aligning on audio…");
            let al = sync::align(
                &sync::decode_audio(&args.cam_a)?,
                &sync::decode_audio(&args.cam_b)?,
                args.max_offset,
            )?;
            println!(
                "  offset {:+.3}s  confidence {:.2}  ambiguity {:.2}",
                al.offset_seconds, al.confidence, al.ambiguity
            );
            if !al.is_trustworthy() {
                eprintln!(
                    "  warning: this alignment is not convincing. Check it in the \
                     editor before triangulating, or re-run with --offset."
                );
            }
            (
                al.offset_seconds,
                "audio_xcorr",
                al.confidence,
                al.ambiguity,
            )
        }
    };

    // The shot clock is A's clock. It can only start where both cameras were
    // already rolling, and can only end where the first of them stopped.
    let start = args.start.unwrap_or(0.0).max(0.0).max(-offset);
    let end = args
        .end
        .unwrap_or(f64::INFINITY)
        .min(a.duration)
        .min(b.duration - offset);
    if end <= start {
        return Err(format!(
            "the two cameras do not overlap: usable window is {start:.2}s to {end:.2}s"
        )
        .into());
    }
    let count = (((end - start) * args.rate).floor() as u32).max(1);

    // Pairing can never be tighter than half a source frame on each camera.
    let precision_ms = (0.5 / a.fps + 0.5 / b.fps) * 1000.0;

    let root = args.out.join(format!("{}.shot", args.name));
    let width = args.scale.unwrap_or(a.width.max(b.width));
    guard_space(&args.out, count, width, a.height, args.force)?;

    let score = match &args.score_audio {
        None => None,
        Some(path) => {
            println!("aligning the take to the score recording…");
            let al = sync::align(
                &sync::decode_audio(&args.cam_a)?,
                &sync::decode_audio(path)?,
                args.max_offset.max(30.0),
            )?;
            println!(
                "  score offset {:+.3}s  confidence {:.2}",
                al.offset_seconds, al.confidence
            );
            Some((path.clone(), al))
        }
    };

    let manifest = Manifest {
        name: args.name.clone(),
        created_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        sync: Sync {
            offset_seconds: offset,
            method: method.to_owned(),
            confidence,
            ambiguity,
            pair_precision_ms: precision_ms,
        },
        score: None,
        subjects: Vec::new(),
        cameras: vec![
            camera("a", &args.cam_a, &a),
            camera("b", &args.cam_b, &b),
        ],
        passes: Vec::new(),
    };
    let mut shot = Shot::create(&root, manifest)?;

    if let Some((path, al)) = score {
        shot.manifest.score = Some(metal_shot::ScoreLink {
            reference: path.display().to_string(),
            offset_seconds: al.offset_seconds,
            confidence: al.confidence,
        });
    }

    println!(
        "extracting {count} pairs at {} fps over {start:.2}s–{end:.2}s",
        args.rate
    );
    extract_pass(
        &mut shot,
        "base",
        start,
        args.rate,
        count,
        offset,
        args.quality,
        args.scale,
    )?;

    shot.save()?;
    report(&shot);
    Ok(())
}

fn window(args: Window) -> Result<(), Box<dyn std::error::Error>> {
    let mut shot = Shot::open(&args.shot)?;
    if shot.pass(&args.id).is_some() {
        return Err(format!("a pass named {:?} already exists", args.id).into());
    }
    if args.end <= args.start {
        return Err("--end must be after --start".into());
    }
    let count = (((args.end - args.start) * args.rate).floor() as u32).max(1);
    let width = args
        .scale
        .unwrap_or_else(|| shot.camera("a").map(|c| c.width).unwrap_or(1920));
    let height = shot.camera("a").map(|c| c.height).unwrap_or(1080);
    guard_space(shot.root(), count, width, height, args.force)?;

    let offset = shot.manifest.sync.offset_seconds;
    println!("extracting {count} pairs at {} fps", args.rate);
    extract_pass(
        &mut shot,
        &args.id,
        args.start,
        args.rate,
        count,
        offset,
        args.quality,
        args.scale,
    )?;
    shot.save()?;
    report(&shot);
    Ok(())
}

/// Extracts both angles onto the shot clock.
///
/// This is where the offset is spent: camera A is sampled at shot time, camera
/// B at shot time plus the offset. Everything downstream can then treat frame
/// `i` of both as one instant.
#[allow(clippy::too_many_arguments)]
fn extract_pass(
    shot: &mut Shot,
    id: &str,
    start: f64,
    rate: f64,
    count: u32,
    offset: f64,
    quality: u8,
    scale: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let sources: Vec<(String, PathBuf, f64, f64)> = shot
        .manifest
        .cameras
        .iter()
        .map(|c| {
            let camera_start = if c.id == "b" { start + offset } else { start };
            (c.id.clone(), PathBuf::from(&c.source), camera_start, c.fps)
        })
        .collect();

    let mut written = u32::MAX;
    for (cam, source, camera_start, fps) in sources {
        let dest = shot.frame_dir(id, &cam);
        video::extract(&source, &dest, camera_start, rate, count, fps, quality, scale, "")?;
        let got = video::count_frames(&dest);
        println!("  {cam}: {got} frames");
        written = written.min(got);
    }

    if written == 0 {
        return Err("ffmpeg produced no frames".into());
    }
    if written < count {
        // A pass is only as long as its shorter half, or the pairing breaks.
        println!("  trimming pass to {written} pairs (one angle ran out first)");
    }

    shot.manifest.passes.push(Pass {
        id: id.to_owned(),
        start,
        rate,
        count: written,
    });
    Ok(())
}

fn camera(id: &str, path: &Path, info: &video::Info) -> Camera {
    Camera {
        id: id.to_owned(),
        source: metal_vfs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string(),
        width: info.width,
        height: info.height,
        fps: info.fps,
        duration: info.duration,
    }
}

/// Refuses to fill the disk. A one-core VPS with 27 GB free is one careless
/// `--rate 30` away from having no room for the next take.
fn guard_space(
    at: &Path,
    count: u32,
    width: u32,
    height: u32,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let per_frame = (width as u64 * height as u64) / 8;
    let needed = per_frame * count as u64 * 2;
    let Some(available) = video::available_bytes(at) else {
        return Ok(());
    };
    println!(
        "  estimated {} MB, {} MB free",
        needed / 1_000_000,
        available / 1_000_000
    );
    if needed > available * 8 / 10 && !force {
        return Err(format!(
            "this pass would take about {} MB and only {} MB is free.\n  \
             Lower --rate, add --scale 1280, or pass --force.",
            needed / 1_000_000,
            available / 1_000_000
        )
        .into());
    }
    Ok(())
}

fn info(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    report(&Shot::open(root)?);
    Ok(())
}

fn report(shot: &Shot) {
    let m = &shot.manifest;
    println!("\n{}  ({})", m.name, shot.root().display());
    println!(
        "sync   {:+.3}s by {}  confidence {:.2}  ambiguity {:.2}  pairing ±{:.0} ms",
        m.sync.offset_seconds, m.sync.method, m.sync.confidence, m.sync.ambiguity, m.sync.pair_precision_ms
    );
    if let Some(s) = &m.score {
        println!(
            "score  {:+.3}s to {}  confidence {:.2}",
            s.offset_seconds, s.reference, s.confidence
        );
    }
    for c in &m.cameras {
        println!("cam {}  {}x{} @ {:.3} fps", c.id, c.width, c.height, c.fps);
    }
    for p in &m.passes {
        print!(
            "pass {:<6} {:>6} pairs  {:.2}s–{:.2}s @ {} fps",
            p.id,
            p.count,
            p.start,
            p.end(),
            p.rate
        );
        match shot.pair_error_stats(&p.id) {
            Some((worst, mean)) => println!(
                "  simultaneity: mean {:.1} ms, worst {:.1} ms",
                mean * 1000.0,
                worst * 1000.0
            ),
            None => println!(),
        }
    }
    println!("size   {} MB", shot.size_bytes() / 1_000_000);
}

fn pack(root: &Path, out: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let root = metal_vfs::canonicalize(&root)?;
    let parent = root.parent().ok_or("shot has no parent directory")?;
    let name = root.file_name().ok_or("shot has no name")?;
    let out = out.unwrap_or_else(|| parent.join(format!("{}.tar.zst", name.to_string_lossy())));

    let status = Command::new("tar")
        .arg("--zstd")
        .arg("-cf")
        .arg(&out)
        .arg("-C")
        .arg(parent)
        .arg(name)
        .status()?;
    if !status.success() {
        return Err("tar failed".into());
    }
    println!(
        "{} ({} MB)",
        out.display(),
        metal_vfs::metadata(&out)?.len() / 1_000_000
    );
    Ok(())
}
