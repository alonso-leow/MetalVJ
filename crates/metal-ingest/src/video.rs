//! ffprobe and ffmpeg, wrapped just enough.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Stdio};
use std::sync::OnceLock;
use metal_schema::platform::Command;

#[derive(Debug, Clone)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
    pub pix_fmt: String,
    pub color_space: String,
}

pub fn probe(path: &Path) -> Result<Info, VideoError> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate,pix_fmt,color_space:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|e| VideoError::Spawn(e.to_string()))?;

    if !out.status.success() {
        return Err(VideoError::Probe(
            path.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| VideoError::Probe(path.display().to_string(), e.to_string()))?;
    let stream = json["streams"]
        .get(0)
        .ok_or_else(|| VideoError::NoVideo(path.display().to_string()))?;

    let fps = stream["avg_frame_rate"]
        .as_str()
        .and_then(parse_ratio)
        .filter(|f| *f > 0.0)
        .ok_or_else(|| VideoError::NoVideo(path.display().to_string()))?;

    Ok(Info {
        width: stream["width"].as_u64().unwrap_or(0) as u32,
        height: stream["height"].as_u64().unwrap_or(0) as u32,
        fps,
        duration: json["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        pix_fmt: stream["pix_fmt"].as_str().unwrap_or_default().to_owned(),
        color_space: stream["color_space"].as_str().unwrap_or_default().to_owned(),
    })
}

/// The colour conversion a YCGCO clip needs before anything else in the graph.
///
/// Resolume's DXV (with alpha) decodes to `yuva420p`/`ycgco`, and swscale does
/// not know YCGCO at all: every filter that touches the pixels — fps, scale,
/// the encoder's own format negotiation — fails, so the clip never becomes a
/// proxy. libzimg's `zscale` does handle it, so this prefix is dropped in front
/// of the chain that follows.
pub const ZSCALE_YCGCO: &str = "zscale=matrixin=ycgco:matrix=bt709:rangein=full:range=limited,";

/// Whether a clip cannot go through swscale — a DXV alpha export, or a stream
/// ffprobe states as YCGCO whatever its pixel format.
pub fn needs_zscale(pix_fmt: &str, color_space: &str) -> bool {
    pix_fmt.contains("yuva") || color_space.eq_ignore_ascii_case("ycgco")
}

/// Whether this ffmpeg build carries `zscale`. Probed once and remembered: the
/// binary on the machine does not change under a running process, and the
/// answer is asked on every clip.
pub fn zscale_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("ffmpeg")
            .args(["-hide_banner", "-filters"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains("zscale"))
            .unwrap_or(false)
    })
}

/// The prefix a clip's filter chain must carry before anything that touches
/// its pixels: [`ZSCALE_YCGCO`] for a hostile source only when `zscale` is
/// there to do the work, and nothing otherwise — a normal clip's chain stays
/// byte-for-byte what it was.
pub fn normalize_prefix(pix_fmt: &str, color_space: &str) -> &'static str {
    if needs_zscale(pix_fmt, color_space) && zscale_available() {
        ZSCALE_YCGCO
    } else {
        ""
    }
}

/// Whether a pixel format carries an alpha channel. Matched by name prefix,
/// not `contains('a')`: `gray` has an `a` and no alpha. DXV's alpha export is
/// `yuva420p`.
pub fn has_alpha(pix_fmt: &str) -> bool {
    ["yuva", "rgba", "bgra", "argb", "abgr", "gbrap", "ya"]
        .iter()
        .any(|p| pix_fmt.starts_with(p))
}

/// ffprobe reports frame rates as exact rationals — `30000/1001`, not 29.97.
fn parse_ratio(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d != 0.0).then_some(n / d)
}

/// Extracts `count` frames at `rate` fps starting at `start` seconds.
///
/// The obvious spelling of this is `-vf fps=R`, and it is wrong here. That
/// filter snaps its output onto absolute multiples of `1/R` in the source
/// timeline, so asking for 1.400 s yields 1.500 s. Each camera is seeked to a
/// different point — B is offset from A by whatever the audio alignment
/// measured — so the two get snapped by *different* amounts, and the pairing
/// this whole format promises quietly stops being true. Measured on a test
/// rig: a 20 ms systematic skew between angles that no amount of care
/// downstream could have recovered.
///
/// So frames are selected by source timestamp instead. For each target time
/// the expression keeps the single source frame nearest to it, which means the
/// only error left is each camera's own frame quantisation — a bound that is
/// known in advance and recorded in the manifest.
#[allow(clippy::too_many_arguments)]
pub fn extract(
    source: &Path,
    dest: &Path,
    start: f64,
    rate: f64,
    count: u32,
    source_fps: f64,
    quality: u8,
    scale_width: Option<u32>,
    prefix: &str,
) -> Result<(), VideoError> {
    metal_vfs::create_dir_all(dest).map_err(|e| VideoError::Spawn(e.to_string()))?;

    let select = select_expr(start, rate, source_fps);
    let filter = extract_filter(prefix, &select, scale_width);

    // A margin before the first target, so the fast seek cannot overshoot it.
    let seek = (start - 1.0).max(0.0);

    let out = Command::new("ffmpeg")
        .args([
            "-v", "error", "-y", "-copyts", // `t` must stay on the source clock
            "-ss", &format!("{seek:.6}"),
        ])
        .arg("-i")
        .arg(source)
        .args([
            "-vf",
            &filter,
            "-fps_mode",
            "passthrough", // never invent or drop frames
            "-frames:v",
            &count.to_string(),
            "-q:v",
            &quality.to_string(),
            "-f",
            "image2",
        ])
        .arg(dest.join("%06d.jpg"))
        .output()
        .map_err(|e| VideoError::Spawn(e.to_string()))?;

    if !out.status.success() {
        return Err(VideoError::Extract(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}

/// The whole filter chain [`extract`] hands ffmpeg: the normalize prefix (empty
/// for an ordinary clip), the select expression, and the optional scale. Its
/// own function so the assembly can be checked without an ffmpeg run.
pub fn extract_filter(prefix: &str, select: &str, scale_width: Option<u32>) -> String {
    match scale_width {
        // -2 keeps the aspect ratio and an even height, which JPEG needs.
        Some(w) => format!("{prefix}{select},scale={w}:-2"),
        None => format!("{prefix}{select}"),
    }
}

/// 2026-10-01: a JPEG ladder (`dir/%06d.jpg`) → one HAP `.mov`, frame `k` =
/// rung `k`. `alpha` picks `hap_alpha` (BC3) over `hap` (BC1). HAP needs both
/// sides a multiple of four, so the picture is cropped to that (at most three
/// pixels), never stretched. Returns the frame size and each packet's
/// `(offset, length)` in the file, which is all a reader needs to seek.
pub fn encode_hap(ladder: &Path, rate: f64, dest: &Path, alpha: bool) -> Result<(u32, u32, Vec<(u64, u32)>), VideoError> {
    if let Some(dir) = dest.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| VideoError::Spawn(e.to_string()))?;
    }
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-framerate", &format!("{rate}")])
        .arg("-i")
        .arg(ladder.join("%06d.jpg"))
        .args([
            "-vf", "crop=trunc(iw/4)*4:trunc(ih/4)*4:0:0",
            "-c:v", "hap", "-format", if alpha { "hap_alpha" } else { "hap" },
            "-chunks", "4", "-compressor", "snappy", "-f", "mov",
        ])
        .arg(dest)
        .output()
        .map_err(|e| VideoError::Spawn(e.to_string()))?;
    if !out.status.success() {
        return Err(VideoError::Encode(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    let info = probe(dest)?;
    let packets = packets(dest)?;
    Ok((info.width, info.height, packets))
}

/// Each video packet's `(offset, length)` in the file, via `ffprobe`, in
/// presentation order — all a reader needs to seek. Shared by both HAP
/// encoders, which differ only in where the frames come from.
fn packets(dest: &Path) -> Result<Vec<(u64, u32)>, VideoError> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pos,size", "-of", "csv=p=0"])
        .arg(dest)
        .output()
        .map_err(|e| VideoError::Spawn(e.to_string()))?;
    if !out.status.success() {
        return Err(VideoError::Probe(dest.display().to_string(), String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split(',');
            let size: u32 = it.next()?.trim().parse().ok()?;
            let pos: u64 = it.next()?.trim().parse().ok()?;
            Some((pos, size))
        })
        .collect())
}

/// 2026-10-01: the source file → one HAP `.mov` in a single pass, alpha and
/// all. This is [`encode_hap`] fed the source instead of the JPEG ladder: a
/// JPEG rung has no alpha, so a Resolume DXV alpha export (`yuva420p`, YCGCO)
/// that goes through the ladder loses its transparency, and `-format hap_alpha`
/// (BC3) has nothing to carry. `alpha` picks BC3 over BC1; `prefix` is the
/// [`normalize_prefix`] the clip needs before any filter touches its pixels (a
/// hostile DXV source carries `zscale` here, the ladder path carries it in the
/// extract chain). HAP needs both sides a multiple of four, so the picture is
/// cropped to that (at most three pixels), never stretched. `total` is the
/// frame count the caller expects — it only scales the progress fraction, and
/// `on` is called with `frame/total` as ffmpeg reports each frame.
#[allow(clippy::too_many_arguments)]
pub fn encode_hap_video(
    source: &Path,
    rate: f64,
    width: u32,
    total: u32,
    dest: &Path,
    prefix: &str,
    alpha: bool,
    on: &mut dyn FnMut(f32),
) -> Result<(u32, u32, Vec<(u64, u32)>), VideoError> {
    if let Some(dir) = dest.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| VideoError::Spawn(e.to_string()))?;
    }
    let filter =
        format!("{prefix}fps={rate},scale={width}:-2,crop=trunc(iw/4)*4:trunc(ih/4)*4:0:0");
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-nostats", "-progress", "pipe:1"])
        .arg("-i")
        .arg(source)
        .args([
            "-an",
            "-vf",
            &filter,
            "-c:v",
            "hap",
            "-format",
            if alpha { "hap_alpha" } else { "hap" },
            "-chunks",
            "4",
            "-compressor",
            "snappy",
            "-f",
            "mov",
        ])
        .arg(dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VideoError::Spawn(e.to_string()))?;

    // `-progress pipe:1` writes `key=value` lines as frames are encoded; only
    // `frame=` is wanted, and only when the caller gave a total to divide by.
    let stdout = child.stdout.take().ok_or_else(|| VideoError::Spawn("no stdout".into()))?;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if total > 0 {
            if let Some(n) = line.strip_prefix("frame=").and_then(|v| v.trim().parse::<u32>().ok())
            {
                on((n as f32 / total as f32).clamp(0.0, 1.0));
            }
        }
    }

    // `wait_with_output` after draining stdout collects the status and the
    // stderr still piped; stdout is already consumed and comes back empty.
    let out = child.wait_with_output().map_err(|e| VideoError::Spawn(e.to_string()))?;
    if !out.status.success() {
        return Err(VideoError::Encode(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    let info = probe(dest)?;
    let packets = packets(dest)?;
    Ok((info.width, info.height, packets))
}

/// Keeps the frame whose own index equals the index of the frame nearest the
/// target time it would be assigned to. Exactly one source frame satisfies
/// this per target, as long as we sample below the source rate.
///
/// The `gte` floor is not optional: the grid described here is periodic and
/// extends backwards, so without it the expression also matches frames inside
/// the seek margin and the pass begins a second early.
fn select_expr(start: f64, rate: f64, fps: f64) -> String {
    format!(
        "select='gte(t\\,{floor:.9})*eq(round(t*{fps:.9})\\,round(({start:.9}+round((t-{start:.9})*{rate:.9})/{rate:.9})*{fps:.9}))'",
        floor = start - 0.5 / fps,
    )
}

/// The source time the extractor will actually land on for target `k`.
/// Mirrors `metal_shot::Shot::realized_time`; kept here so the expression and
/// the prediction can be checked against each other.
pub fn realized(start: f64, rate: f64, fps: f64, k: u32) -> f64 {
    (((start + k as f64 / rate) * fps).round()) / fps
}

pub fn count_frames(dir: &Path) -> u32 {
    metal_vfs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
                .count() as u32
        })
        .unwrap_or(0)
}

/// Free bytes on the filesystem holding `path`, via `df`.
pub fn available_bytes(path: &Path) -> Option<u64> {
    let out = Command::new("df").args(["-Pk"]).arg(path).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let avail: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail * 1024)
}

/// Encodes RGBA8 frames, pulled one at a time from `next_frame` until it
/// returns `None`, into an H.264 MP4 at `dest`.
///
/// This is `export.video`'s ffmpeg — `the design notes` Conjunto G4 — and
/// the direction is the mirror of [`extract`]: frames in, one video out,
/// rather than one video in, frames out. It belongs beside `extract` rather
/// than in `metal-dash` because the ffmpeg invocation is the thing this module
/// exists to own once for the whole workspace (see the crate doc comment);
/// `metal-dash` only decides *what* the frames are.
///
/// # Determinism is the point, not a side effect
///
/// G4's hard requirement is that two runs over the same frames produce the
/// same file, byte for byte — that guarantee is the entire reason
/// `export.video` is a separate `Offline` domain instead of a live "record"
/// button on the compositor (see `Domain` in `metal-graph`), so it is worth
/// spelling out what makes it hold rather than hoping libx264 is well
/// behaved:
///
/// - **`-metadata creation_time=...` is pinned to the epoch.** Left to the
///   muxer, an mp4's `mvhd`/`mdhd` atoms can be stamped with the wall clock
///   the file was written, which would make the same export produce two
///   different files a minute apart before a single pixel was considered.
/// - **`-fflags +bitexact -flags:v +bitexact`** turns off the muxer's
///   free-form "written by Lavf x.y.z" strings and any other writer-identity
///   metadata that is a property of the machine and moment, not of the
///   frames.
/// - **`-threads 1`** forces single-threaded encoding. Measured on this
///   workspace's ffmpeg (`ffmpeg -version` reports the exact build), a
///   multi-threaded `libx264` run over identical rawvideo input already
///   produced identical output both with and without the flags above — but
///   x264's own documentation reserves the right to let frame-threaded
///   lookahead vary its internal decisions with thread count, and
///   `export.video` runs offline with no realtime deadline to protect, so
///   there is nothing to buy by leaving that door open. Pin it rather than
///   rely on today's build happening to be well behaved.
///
/// `tests::two_encodes_of_the_same_frames_are_byte_identical` is what proved
/// this rather than assumed it — see that test for the measurement.
pub fn encode_rgba(
    dest: &Path,
    width: u32,
    height: u32,
    fps: f64,
    mut next_frame: impl FnMut() -> Option<Vec<u8>>,
) -> Result<(), VideoError> {
    if width == 0 || height == 0 {
        return Err(VideoError::Encode(format!("{width}x{height} is not a frame size")));
    }
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
        .args(["-s", &format!("{width}x{height}")])
        .args(["-r", &format!("{fps}")])
        .args(["-i", "-"])
        .args(["-an", "-threads", "1"])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-fflags", "+bitexact", "-flags:v", "+bitexact"])
        .args(["-metadata", "creation_time=1970-01-01T00:00:00Z"])
        .arg(dest)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VideoError::Spawn(e.to_string()))?;

    // Taken out of the child so writing frames and the eventual `wait` do not
    // both need `&mut child` at once. Dropping it — here on an early error,
    // or below once every frame is written — is what sends ffmpeg its
    // end-of-stream; without an explicit drop before `wait_with_output`, the
    // process would sit waiting for more input forever.
    let mut stdin = child.stdin.take().ok_or_else(|| VideoError::Spawn("no stdin".into()))?;
    let expect = (width as usize) * (height as usize) * 4;

    let write_result: Result<(), VideoError> = (|| {
        while let Some(frame) = next_frame() {
            if frame.len() != expect {
                return Err(VideoError::Encode(format!(
                    "frame is {} bytes, expected {expect} for {width}x{height} rgba",
                    frame.len()
                )));
            }
            stdin.write_all(&frame).map_err(|e| VideoError::Spawn(e.to_string()))?;
        }
        Ok(())
    })();
    drop(stdin);

    let out = child.wait_with_output().map_err(|e| VideoError::Spawn(e.to_string()))?;
    // A write failure (ffmpeg exited early — a bad `dest` path, a codec this
    // build lacks) and a non-zero exit both end here; whichever the caller
    // sees, ffmpeg's own stderr is the honest diagnosis and is preferred over
    // a bare "broken pipe" from the write side whenever ffmpeg left one.
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if !out.status.success() {
        return Err(VideoError::Encode(if stderr.is_empty() {
            format!("ffmpeg exited with {}", out.status)
        } else {
            stderr
        }));
    }
    write_result?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("could not run ffmpeg/ffprobe: {0}")]
    Spawn(String),
    #[error("could not probe {0}: {1}")]
    Probe(String, String),
    #[error("{0} has no video stream")]
    NoVideo(String),
    #[error("frame extraction failed: {0}")]
    Extract(String),
    #[error("could not encode video: {0}")]
    Encode(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates_are_read_as_exact_rationals() {
        assert_eq!(parse_ratio("30000/1001"), Some(30000.0 / 1001.0));
        assert_eq!(parse_ratio("25/1"), Some(25.0));
        assert_eq!(parse_ratio("0/0"), None);
        assert_eq!(parse_ratio("25"), None);
    }

    /// The set of clips swscale cannot touch: DXV's alpha exports (`yuva…`),
    /// and anything ffprobe states as YCGCO whatever its pixel format.
    #[test]
    fn hostile_sources_are_the_ones_that_need_zscale() {
        assert!(needs_zscale("yuva420p", "ycgco"));
        assert!(needs_zscale("yuva420p", "unknown"));
        assert!(needs_zscale("yuv420p", "ycgco"));
        assert!(needs_zscale("yuv420p", "YCGCO"), "ffprobe is not case-stable");
        assert!(!needs_zscale("yuv420p", "bt709"));
    }

    /// Which pixel formats carry alpha, by name family. The trap is `gray`:
    /// matched with `contains('a')` it would claim an alpha channel it has not
    /// got, and the whole point of the prefix list is to avoid exactly that.
    #[test]
    fn alpha_pixel_formats_are_the_ones_with_an_alpha_channel() {
        for yes in ["yuva420p", "yuva444p10le", "rgba", "bgra", "argb", "abgr", "ya8"] {
            assert!(has_alpha(yes), "{yes} carries alpha");
        }
        for no in ["yuv420p", "yuvj420p", "rgb24", "gray", "gray16le", "nv12"] {
            assert!(!has_alpha(no), "{no} carries no alpha");
        }
    }

    /// The prefix is only spent when the build can actually do the work; and a
    /// normal clip's chain is unchanged either way.
    #[test]
    fn the_prefix_leads_the_chain_and_leaves_an_ordinary_clip_alone() {
        let select = "select='x'";
        let prefixed = extract_filter(ZSCALE_YCGCO, select, Some(480));
        assert!(prefixed.starts_with(ZSCALE_YCGCO));
        assert!(prefixed.contains(select));
        assert!(prefixed.contains("scale=480:-2"));

        let plain = extract_filter("", select, Some(480));
        assert_eq!(plain, format!("{select},scale=480:-2"));
        assert_eq!(extract_filter("", select, None), select);

        // Whatever the machine's ffmpeg, a normal clip never gets a prefix.
        assert_eq!(normalize_prefix("yuv420p", "bt709"), "");
    }

    #[test]
    fn free_space_is_readable() {
        assert!(available_bytes(Path::new(".")).is_some_and(|b| b > 0));
    }

    /// The correctness-critical one: does the select expression actually land
    /// on the frames we predicted?
    ///
    /// This caught a 20 ms systematic skew between angles that had already
    /// passed every other check, so it builds real footage and reads back the
    /// timestamps ffmpeg chose rather than trusting the filter.
    #[test]
    fn selection_lands_on_the_frames_we_predict() {
        let dir = std::env::temp_dir().join(format!("fw-select-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        // Two cameras with different frame rates, as in a real two-angle shoot.
        for (name, fps) in [("a", 30.0), ("b", 25.0)] {
            let ok = Command::new("ffmpeg")
                .args(["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("testsrc2=size=160x90:rate={fps}:duration=8"))
                .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
                .arg(dir.join(format!("{name}.mp4")))
                .status();
            if !ok.is_ok_and(|s| s.success()) {
                eprintln!("skipping: ffmpeg unavailable");
                return;
            }
        }

        for (name, fps, start, rate) in [
            ("a", 30.0, 1.4, 4.0),   // start off the sampling grid
            ("b", 25.0, 0.0, 4.0),   // start on it
            ("a", 30.0, 2.137, 3.0), // an ugly start and a non-divisor rate
        ] {
            let out = Command::new("ffmpeg")
                .args(["-hide_banner", "-copyts", "-ss"])
                .arg(format!("{:.6}", (start - 1.0f64).max(0.0)))
                .arg("-i")
                .arg(dir.join(format!("{name}.mp4")))
                .args([
                    "-vf",
                    &format!("{},showinfo", select_expr(start, rate, fps)),
                    "-fps_mode",
                    "passthrough",
                    "-frames:v",
                    "6",
                    "-f",
                    "null",
                    "-",
                ])
                .output()
                .unwrap();

            let got: Vec<f64> = String::from_utf8_lossy(&out.stderr)
                .split("pts_time:")
                .skip(1)
                .filter_map(|s| {
                    s.split(|c: char| !(c.is_ascii_digit() || c == '.'))
                        .next()?
                        .parse()
                        .ok()
                })
                .take(6)
                .collect();

            assert_eq!(got.len(), 6, "{name}: expected 6 frames, got {got:?}");
            for (k, actual) in got.iter().enumerate() {
                let want = realized(start, rate, fps, k as u32);
                assert!(
                    (actual - want).abs() < 1e-6,
                    "{name} @{start}/{rate}: frame {k} landed at {actual}, predicted {want}"
                );
                // And the prediction is always within half a source frame of
                // the time actually asked for.
                let asked = start + k as f64 / rate;
                assert!((want - asked).abs() <= 0.5 / fps + 1e-9);
            }
        }
        metal_vfs::remove_dir_all(&dir).unwrap();
    }

    // -- G4: export.video's determinism guarantee ---------------------------

    /// Deterministic synthetic frames — not random, and not a solid colour
    /// either: every pixel differs by position and by frame index, which is
    /// what makes a byte-for-byte comparison of the encoded output mean
    /// something rather than passing by coincidence on a uniform input.
    fn synthetic_frames(w: u32, h: u32, n: u32) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                let mut buf = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h {
                    for x in 0..w {
                        buf.extend_from_slice(&[
                            ((x + i * 7) & 255) as u8,
                            ((y + i * 13) & 255) as u8,
                            ((x ^ y ^ i) & 255) as u8,
                            255,
                        ]);
                    }
                }
                buf
            })
            .collect()
    }

    /// G4's hard requirement, proved rather than assumed: two encodes of the
    /// exact same frames must produce the exact same file. This is the
    /// property the whole `Offline` domain is justified by — if it did not
    /// hold, "export" would just be a slower, less honest "record".
    ///
    /// A one-second gap is inserted between the two runs specifically to
    /// catch a wall-clock timestamp leaking into the container, which is the
    /// realistic way this guarantee breaks (see `encode_rgba`'s doc comment).
    #[test]
    fn two_encodes_of_the_same_frames_are_byte_identical() {
        let dir = std::env::temp_dir().join(format!("fw-encode-det-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        let (w, h, n) = (48u32, 32u32, 6u32);
        let frames = synthetic_frames(w, h, n);

        let run = |name: &str| -> Option<std::path::PathBuf> {
            let dest = dir.join(name);
            let mut it = frames.clone().into_iter();
            match encode_rgba(&dest, w, h, 8.0, || it.next()) {
                Ok(()) => Some(dest),
                Err(e) => {
                    eprintln!("skipping: ffmpeg unavailable or refused ({e})");
                    None
                }
            }
        };

        let Some(out1) = run("a.mp4") else { return };
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let Some(out2) = run("b.mp4") else { return };

        let (bytes1, bytes2) = (metal_vfs::read(&out1).unwrap(), metal_vfs::read(&out2).unwrap());
        assert!(!bytes1.is_empty(), "an encode that reports success must leave a real file");
        assert_eq!(
            bytes1, bytes2,
            "two encodes of the same {n} frames produced different files \
             ({} vs {} bytes) — the Offline domain's determinism guarantee does not hold",
            bytes1.len(),
            bytes2.len()
        );
        metal_vfs::remove_dir_all(&dir).unwrap();
    }

    /// A frame of the wrong size is refused rather than fed to ffmpeg, which
    /// would otherwise either hang waiting for the rest of a short frame or
    /// silently misinterpret where one frame ends and the next begins.
    #[test]
    fn a_mis_sized_frame_is_refused_not_silently_reshaped() {
        let dir = std::env::temp_dir().join(format!("fw-encode-badsize-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.mp4");

        let mut sent = false;
        let err = encode_rgba(&dest, 16, 16, 8.0, || {
            if sent {
                None
            } else {
                sent = true;
                Some(vec![0u8; 10]) // nowhere near 16*16*4
            }
        });
        match err {
            Err(VideoError::Encode(msg)) => assert!(msg.contains("bytes, expected"), "{msg}"),
            other => {
                // ffmpeg missing entirely reports as `Spawn`, which is a
                // different honest failure and not this test's concern.
                if let Err(VideoError::Spawn(_)) = other {
                    eprintln!("skipping: ffmpeg unavailable");
                } else {
                    panic!("expected an Encode size error, got {other:?}");
                }
            }
        }
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_sized_frames_are_refused_before_spawning_anything() {
        let err = encode_rgba(Path::new("/dev/null"), 0, 10, 8.0, || None).unwrap_err();
        assert!(matches!(err, VideoError::Encode(_)));
    }
}
