//! `metal-listen` — look at what the digester is hearing, without a dashboard.
//!
//! Commands, all for answering "is it actually working":
//!
//! ```text
//! metal-listen devices                     # what can be listened to
//! metal-listen listen [name]               # print the sixteen columns, live
//! metal-listen repeat <song.8hz.npy>       # the loop detector, one line a beat
//! metal-listen digest <song> [out.npy]     # digest_audio.py, in Rust (feature decode)
//! metal-listen digest <song> --tone        # ...with the tone block (feature tone)
//! metal-listen tone <song>                 # mean of each tone column, is a model here
//! ```
//!
//! `devices` and `listen` need the capture feature, which is not on by default;
//! `repeat` reads a shipped digest and does not. `--tone` and `tone` need both
//! `decode` and `tone`:
//!
//! ```text
//! cargo run -p metal-listen --features capture -- devices
//! cargo run -p metal-listen -- repeat data/audio/<song>.8hz.npy
//! cargo run -p metal-listen --features decode -- digest <song> --tone
//! cargo run -p metal-listen --features decode -- tone <song>
//! ```
//!
//! On a checkout without `data/models/basic-pitch/model.onnx` the tone columns
//! are all zero and `tone` reports the model as not loaded (D9) — that is the
//! honest answer, not a failure of the digest.

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("listen");
    match cmd {
        "devices" => devices(),
        "listen" => listen(args.get(1).map(String::as_str)),
        "repeat" => match args.get(1) {
            Some(path) => repeat(path),
            None => {
                eprintln!("repeat needs a file: metal-listen repeat <song>.npy");
                std::process::exit(2);
            }
        },
        "digest" => match args.get(1) {
            Some(path) => {
                // `[out.npy] [--tone]` in either order: the flag is whatever
                // starts with `--`, the output is the one positional.
                let rest = &args[2..];
                let tone = rest.iter().any(|a| a == "--tone");
                let out = rest.iter().find(|a| !a.starts_with("--"));
                digest(path, out.map(String::as_str), tone);
            }
            None => {
                eprintln!("digest needs a file: metal-listen digest <song> [out.32hz.npy] [--tone]");
                std::process::exit(2);
            }
        },
        // Step 7: digest with the tone block and print what came out, so a
        // missing model is visible as zeros rather than silent.
        "tone" => match args.get(1) {
            Some(path) => tone(path),
            None => {
                eprintln!("tone needs a file: metal-listen tone <song>");
                std::process::exit(2);
            }
        },
        // Item 8: the corpus re-digest. Writes both levels beside `dir` (or the
        // input's own folder), base taken from a direct base pass so its bytes
        // match the pre-V2b digest.
        "levels" => match args.get(1) {
            Some(path) => levels(path, args.get(2).map(String::as_str)),
            None => {
                eprintln!("levels needs a file: metal-listen levels <song> [dir]");
                std::process::exit(2);
            }
        },
        "-h" | "--help" | "help" => usage(),
        other => {
            eprintln!("unknown command {other:?}");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!(
        "usage: metal-listen [devices | listen [device-name-substring] | repeat <song>.npy | digest <song> [out.32hz.npy] [--tone] | tone <song> | levels <song> [dir]]"
    );
}

#[cfg(feature = "decode")]
fn digest(path: &str, out: Option<&str>, tone: bool) {
    let input = Path::new(path);
    let out = out.map(std::path::PathBuf::from).unwrap_or_else(|| {
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("song");
        input.with_file_name(metal_schema::digest_file_name(&stem, metal_schema::TickRate::CANONICAL))
    });
    if tone {
        #[cfg(feature = "tone")]
        {
            // `TONE_MODEL` is the registry path (D9); a missing file is the
            // supported no-model state and `loaded` says so.
            match metal_listen::file::digest_file_with_tone(input, Some(Path::new(metal_listen::TONE_MODEL))) {
                Ok((rows, loaded)) => {
                    if let Err(e) = metal_listen::file::write_npy(&out, &rows) {
                        eprintln!("{}: {e}", out.display());
                        std::process::exit(1);
                    }
                    println!(
                        "{path}: {} windows ({:.1}s) -> {} (tone model loaded: {loaded})",
                        rows.len(),
                        rows.len() as f32 / metal_schema::TickRate::CANONICAL.hz() as f32,
                        out.display()
                    );
                    return;
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        #[cfg(not(feature = "tone"))]
        {
            eprintln!("--tone needs the tone feature: cargo run -p metal-listen --features decode,tone -- digest <song> --tone");
            std::process::exit(1);
        }
    }
    match metal_listen::file::digest_to_npy(input, &out) {
        Ok(rows) => println!(
            "{path}: {rows} windows ({:.1}s) -> {}",
            rows as f32 / metal_schema::TickRate::CANONICAL.hz() as f32,
            out.display()
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// Digest a song with the tone block and print the **mean** of each of the six
/// tone columns, plus whether the Basic Pitch model was actually loaded.
///
/// The means are unweighted over the whole song. With no model (D9) every
/// column is zero and `loaded` is `false`; the point of the command is to make
/// that plainly visible rather than to show fabricated curves.
#[cfg(all(feature = "decode", feature = "tone"))]
fn tone(path: &str) {
    let input = Path::new(path);
    match metal_listen::file::digest_file_with_tone(input, Some(Path::new(metal_listen::TONE_MODEL))) {
        Ok((rows, loaded)) => {
            let n = rows.len().max(1) as f32;
            let mut mean = [0.0f32; metal_listen::TONE];
            for row in &rows {
                for (c, m) in mean.iter_mut().enumerate() {
                    *m += row[metal_listen::MUSIC + metal_listen::VOICE + c];
                }
            }
            for m in &mut mean {
                *m /= n;
            }
            println!(
                "{path}: {} windows ({:.1}s), model loaded: {loaded}",
                rows.len(),
                rows.len() as f32 / metal_schema::TickRate::CANONICAL.hz() as f32
            );
            for (c, name) in metal_schema::TONE_COLUMNS.iter().enumerate() {
                println!("  {name:<14} {:.4}", mean[c]);
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(all(feature = "decode", feature = "tone")))]
fn tone(_path: &str) {
    eprintln!("metal-listen was built without decode+tone.");
    eprintln!("  cargo run -p metal-listen --features decode,tone -- tone <song>");
    std::process::exit(1);
}

#[cfg(feature = "decode")]
fn levels(path: &str, dir: Option<&str>) {
    let input = Path::new(path);
    let dir = dir
        .map(std::path::PathBuf::from)
        .or_else(|| input.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("song");
    match metal_listen::file::digest_to_levels_direct(input, &dir, stem) {
        Ok((fine, base)) => println!(
            "{path}: {fine} canonical ({:.1}s) + {base} base -> {}",
            fine as f32 / metal_schema::TickRate::CANONICAL.hz() as f32,
            dir.display()
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "decode"))]
fn levels(_path: &str, _dir: Option<&str>) {
    eprintln!("metal-listen was built without decode.");
}

#[cfg(not(feature = "decode"))]
fn digest(_path: &str, _out: Option<&str>, _tone: bool) {
    eprintln!("metal-listen was built without decode.");
    eprintln!("  cargo run -p metal-listen --features decode -- digest <song>");
    std::process::exit(1);
}

#[cfg(not(feature = "capture"))]
fn devices() {
    no_capture()
}

#[cfg(not(feature = "capture"))]
fn listen(_pick: Option<&str>) {
    no_capture()
}

#[cfg(not(feature = "capture"))]
fn no_capture() -> ! {
    eprintln!("metal-listen was built without capture.");
    eprintln!("  cargo run -p metal-listen --features capture -- devices");
    std::process::exit(1);
}

// --- repeat: the loop detector over a shipped digest -----------------------
//
// The corpus ships `<song>.8hz.npy` (a `(rows, 16)` float32 matrix on the 8 Hz
// grid) next to every `<song>.mp3`, and this crate has no file decoder, so the
// measurement runs on the digest rather than on the audio. That is the same
// matrix the live path produces, one row at a time, so the detector sees the
// material it will see in a room.
//
// The digest is 16 columns, not the 18-dim `BeatVector`: it has no separate
// onset sub-beat bins — column 2 is a single onset value per row. A `Native`
// is synthesised per row so `beat_vector` can pool the beat exactly as the
// live path does; with a handful of rows in a beat the onset lands in the
// bin its ordinal position implies. The grid is a fixed 120 BPM (4 rows), not
// the real `Tempo`: the corpus digest carries no tempo, and for the
// measurement every song is read at the same nominal rate. That is what keeps
// the CLI honest — a digest-level readout, not a claim the live path re-ran.

/// The smallest .npy reader that does this job — same shape as
/// `tests/reference.rs`'s, duplicated because a test helper is not a binary's.
struct Npy {
    shape: Vec<usize>,
    data: Vec<f32>,
}

impl Npy {
    fn load(path: &Path) -> Result<Npy, String> {
        let bytes = metal_vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
            return Err(format!("{} is not a .npy", path.display()));
        }
        let (header_len, start) = match bytes[6] {
            1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
            _ => (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12,
            ),
        };
        let header =
            String::from_utf8_lossy(&bytes[start..start + header_len]).to_string();
        if !(header.contains("'<f4'") || header.contains("\"<f4\"")) {
            return Err(format!("{} is not little-endian float32", path.display()));
        }
        let open = header.find('(').ok_or("no shape in header")?;
        let close = header[open..].find(')').ok_or("no shape in header")? + open;
        let shape: Vec<usize> = header[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.parse())
            .collect::<Result<_, _>>()
            .map_err(|e| format!("shape entry: {e}"))?;
        let body = &bytes[start + header_len..];
        let data: Vec<f32> = body
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        Ok(Npy { shape, data })
    }
}

// `repeat` runs on the digester's detector, which is not behind the capture
// feature, so unlike `devices`/`listen` it has one implementation and no stub.
fn repeat(path: &str) {
    use metal_listen::repeat::{beat_vector, ENTER, EXIT, LENGTHS};
    use metal_listen::{Native, RATE};

    let source = Path::new(path);
    let npy = match Npy::load(source) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("cannot read digest: {e}");
            std::process::exit(1);
        }
    };
    if npy.shape.len() != 2 || npy.shape[1] != metal_listen::FEATURES {
        eprintln!(
            "expected a (rows, {}) digest, got shape {:?}",
            metal_listen::FEATURES,
            npy.shape
        );
        std::process::exit(1);
    }
    let rows = npy.shape[0];
    let width = npy.shape[1];

    eprintln!(
        "{}: {} rows ({:.1} s), 16 columns",
        source.display(),
        rows,
        rows as f32 / RATE
    );
    eprintln!("ENTER={ENTER:.2} EXIT={EXIT:.2} LENGTHS={LENGTHS:?}");

    // A single `Digester` is not needed: the digest is already pooled at
    // 8 Hz, so the beat grid is imposed directly and `Repeat` is fed
    // `BeatVector`s. This keeps the measurement exactly on the detector.
    let mut detector = metal_listen::repeat::Repeat::new();

    // 120 BPM => 2 beats/s => one beat every 4 rows of the 8 Hz grid.
    let beat_rows = 4usize;
    let mut locked: Option<u32> = None;
    let mut beat = 0u32;
    let mut n_locked = 0u32;

    for start in (0..rows).step_by(beat_rows) {
        let end = (start + beat_rows).min(rows);
        let frames: Vec<Native> = (start..end)
            .map(|r| {
                let row = &npy.data[r * width..(r + 1) * width];
                let mut chroma = [0.0f32; metal_listen::N_CHROMA];
                chroma.copy_from_slice(&row[4..16]);
                Native {
                    rms: row[0],
                    bandwidth: row[1],
                    centroid: row[3],
                    // Column 2 is the onset for the row; place it in the
                    // sub-beat bin its position implies.
                    onset: row[2],
                    chroma,
                }
            })
            .collect();
        let vector = beat_vector(&frames);
        let state = detector.push(vector);

        let l4 = detector.score(4);
        let shown = match l4 {
            Some(s) => format!("{s:.4}"),
            None => "  --  ".into(),
        };
        match (locked, state) {
            (None, Some(s)) => {
                println!(
                    "LOCK    L={} conf={:.3} since={:.0}",
                    s.length_beats, s.confidence, s.since_beat
                );
                locked = Some(s.length_beats);
                n_locked += 1;
            }
            (Some(old), Some(s)) if old != s.length_beats => {
                println!(
                    "LOCK    L={} conf={:.3} since={:.0}",
                    s.length_beats, s.confidence, s.since_beat
                );
                locked = Some(s.length_beats);
                n_locked += 1;
            }
            (Some(old), None) => {
                println!("RELEASE after L={old}");
                locked = None;
            }
            _ => {}
        }

        println!(
            "beat {beat:>5} bpm 120 score(L4)={shown} locked L={} conf={:.3}",
            locked
                .map(|l| l.to_string())
                .unwrap_or_else(|| "-".into()),
            state.map(|s| s.confidence).unwrap_or(0.0)
        );
        beat += 1;
    }

    println!("--- {beat} beats, {n_locked} lock event(s) ---");
}

#[cfg(feature = "capture")]
fn devices() {
    match metal_listen::capture::devices() {
        Err(e) => {
            eprintln!("cannot enumerate inputs: {e}");
            std::process::exit(1);
        }
        Ok(list) if list.is_empty() => eprintln!("no input devices"),
        Ok(list) => {
            println!("{} input device(s):", list.len());
            for d in &list {
                // The marker is the point of this listing: on a PipeWire box
                // the useful device is the monitor and it is never the default.
                let mark = if d.monitor { "  <- monitor (what is playing)" } else { "" };
                let via = if d.pulse { "pipe " } else { "cpal " };
                println!(
                    "  {via}{:<48} {} ch @ {} Hz{}",
                    d.name, d.channels, d.sample_rate, mark
                );
            }
        }
    }
}

#[cfg(feature = "capture")]
fn listen(pick: Option<&str>) {
    use metal_listen::COLUMNS;

    eprintln!("opening {:?}…", pick.unwrap_or("<default>"));
    let mut listener = match metal_listen::capture::Listener::open(pick) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot open input: {e}");
            eprintln!("try: metal-listen devices");
            std::process::exit(1);
        }
    };
    eprintln!("listening: {}", listener.describe());
    eprintln!("{:>7} {:>7} {:>7} {:>7}  {:<26} {:>9}", "rms", "bandw", "onset", "centr", "chroma", "beats");

    let mut last = [0.0f32; metal_listen::FEATURES];
    let mut ticks = 0u32;
    loop {
        // Faster than the 8 Hz grid on purpose: this is how a dashboard polls,
        // and polling faster than the grid must not change what comes out.
        std::thread::sleep(std::time::Duration::from_millis(40));
        let row = listener.poll();
        ticks += 1;
        // Print unchanged rows about once a second rather than never. Silence
        // is a row that never changes, and skipping it entirely made a stream
        // that was producing nothing look identical to one that had not
        // started — which cost real time to diagnose once already.
        if row == last && ticks % 25 != 0 {
            continue;
        }
        last = row;

        let bar = |v: f32| {
            let n = (v.clamp(0.0, 1.0) * 12.0) as usize;
            let mut s = String::new();
            for i in 0..12 {
                s.push(if i < n { '#' } else { '.' });
            }
            s
        };
        let loudest = (4..16)
            .max_by(|&a, &b| row[a].partial_cmp(&row[b]).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(4);
        let beats = match (listener.beats(), listener.bpm()) {
            (Some(b), Some(bpm)) => format!("{b:7.2} @{bpm:.0}"),
            _ => "   ---".into(),
        };
        println!(
            "{:>7.4} {:>7.0} {:>7.3} {:>7.4}  {} {:<4} {:>9}",
            row[0],
            row[1],
            row[2],
            row[3],
            bar(row[2].min(1.0)),
            COLUMNS[loudest].trim_start_matches("chroma "),
            beats
        );
    }
}
