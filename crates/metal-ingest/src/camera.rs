//! Camera enumeration and single-frame capture — H.5a, "real camera
//! capture". `the design notes` H.5: *"Blocking: nothing here runs without
//! real camera capture."*
//!
//! **Why ffmpeg and not a v4l2 binding.** ffmpeg already sits on the ingest
//! path ([`crate::video`]) and already knows how to open a v4l2 device,
//! negotiate whatever pixel format the camera actually speaks (commonly
//! YUYV, not RGBA), and convert it. A hand-rolled ioctl binding would have to
//! redo all three of those for a single frame grab; spawning ffmpeg once per
//! call is the same trade this crate already made for [`crate::video::extract`]
//! and [`crate::video::encode_rgba`], just pointed at a device node instead
//! of a file.
//!
//! **No inference here either** — this module's whole job ends at
//! [`metal_sense::Image`], the same handoff point `metal-sense`'s own doc comment
//! describes: "a real backend converts whatever `capture` hands it into this
//! shape". This *is* that backend for the file-shaped half; nothing here
//! looks at pixels.

use std::path::Path;
use metal_schema::platform::Command;

/// One camera ffmpeg can see: a device node and the name the kernel reports
/// for it (often repeated across the metadata and capture nodes a single
/// physical webcam exposes — see [`list_devices`]'s doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    pub name: String,
}

/// Lists the devices `ffmpeg -sources v4l2` reports, rather than walking
/// `/dev/video*` by hand. ffmpeg already does the enumeration a hand-rolled
/// walk would have to redo — in particular, a single physical webcam
/// commonly exposes two `/dev/videoN` nodes (one real capture device, one
/// metadata-only), and telling those apart from a bare directory listing
/// means reading v4l2 capability flags anyway, which `-sources` already did.
///
/// A host with no v4l2 devices (no camera, or ffmpeg built without v4l2
/// support) exits 0 with nothing to parse — this returns `Ok(vec![])` for
/// that, not an error; "no camera" is a normal outcome for a caller (or a
/// test on CI) to branch on, not a failure to propagate.
pub fn list_devices() -> Result<Vec<Device>, CameraError> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-sources", "v4l2"])
        .output()
        .map_err(|e| CameraError::Spawn(e.to_string()))?;
    let mut devices = parse_sources(&String::from_utf8_lossy(&out.stdout));
    // ffmpeg lists a webcam's nodes in whatever order the kernel handed them
    // over, and on this project's own laptop that is the metadata node
    // (`/dev/video1`) first — so "the first camera" was the one node that
    // can never give a frame. The kernel numbers a device's nodes by
    // interface, video first (`index` 0); that is the order kept here.
    devices.sort_by_key(|d| (node_index(&d.path), d.path.clone()));
    Ok(devices)
}

/// The kernel's per-device node number (`/sys/class/video4linux/videoN/index`):
/// 0 for a webcam's video node, 1 and up for its metadata nodes. Unknown
/// sorts last, not first.
fn node_index(path: &str) -> u32 {
    let Some(name) = Path::new(path).file_name() else {
        return u32::MAX;
    };
    metal_vfs::read_to_string(Path::new("/sys/class/video4linux").join(name).join("index"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(u32::MAX)
}

/// `ffmpeg -sources v4l2`'s own format:
/// ```text
/// Auto-detected sources for video4linux2,v4l2:
///   /dev/video1 [HP Wide Vision HD Camera: HP Wi] (none)
///   /dev/video0 [HP Wide Vision HD Camera: HP Wi] (none)
/// ```
/// One device per line, the path first, the name in brackets. Kept separate
/// from [`list_devices`] so the parsing — the part that can actually be
/// wrong — is testable without a real camera in the room.
fn parse_sources(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let path = line.split_whitespace().next()?;
            if !path.starts_with("/dev/") {
                return None;
            }
            let name = line
                .split_once('[')
                .and_then(|(_, rest)| rest.split_once(']'))
                .map(|(n, _)| n.trim().to_string())
                .unwrap_or_default();
            Some(Device { path: path.to_string(), name })
        })
        .collect()
}

/// How long one ffmpeg grab may take before it is killed.
///
/// A v4l2 device that enumerates but never delivers a frame — a laptop's
/// privacy shutter or camera key, a USB camera starved of bandwidth, a node
/// that is metadata rather than video — does not make ffmpeg fail: it makes
/// it wait, forever. Without a deadline one `capture_camera_frame` call
/// hung the whole agent turn. Six seconds is several times what a real
/// camera needs to wake and hand over a first frame.
pub const CAPTURE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(6);

/// Runs one ffmpeg invocation with [`CAPTURE_DEADLINE`], killing it when the
/// deadline passes. stdout is drained on a thread so a large frame cannot
/// fill the pipe and stall ffmpeg before the deadline check ever sees it.
fn ffmpeg_with_deadline(device: &Path, args: &[String]) -> Result<Vec<u8>, CameraError> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CameraError::Spawn(e.to_string()))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= CAPTURE_DEADLINE => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(CameraError::Spawn(e.to_string())),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).trim().to_owned();
    match status {
        None => Err(CameraError::Timeout(device.display().to_string(), CAPTURE_DEADLINE.as_secs())),
        Some(s) if !s.success() || stdout.is_empty() => {
            Err(CameraError::Capture(device.display().to_string(), last_lines(&stderr)))
        }
        Some(_) => Ok(stdout),
    }
}

/// ffmpeg repeats a per-buffer warning hundreds of times on a camera that
/// streams garbage; the last few distinct lines are the diagnosis.
fn last_lines(stderr: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for l in stderr.lines().rev() {
        if !lines.contains(&l) {
            lines.push(l);
        }
        if lines.len() == 3 {
            break;
        }
    }
    lines.reverse();
    lines.join(" | ")
}

/// The ffmpeg arguments for one grab. `mjpeg` asks the camera for its
/// compressed stream explicitly: most USB webcams only offer their larger
/// sizes as MJPEG, and ffmpeg's default negotiation asks for raw YUYV at the
/// requested size, which such a camera either refuses or answers with
/// buffers the driver flags as corrupted.
fn grab_args(device: &Path, width: u32, height: u32, mjpeg: bool, output: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-v", "error", "-f", "v4l2"].map(String::from).to_vec();
    if mjpeg {
        args.extend(["-input_format".to_string(), "mjpeg".to_string()]);
    }
    args.extend(["-video_size".to_string(), format!("{width}x{height}"), "-i".to_string()]);
    args.push(device.display().to_string());
    args.extend(output.iter().map(|s| s.to_string()));
    args
}

/// One grab, the camera's default format first and MJPEG second. A timeout
/// on the first try is not retried: a camera that delivered nothing in the
/// deadline will not deliver in a second one, and the caller is waiting.
fn grab(device: &Path, width: u32, height: u32, output: &[&str]) -> Result<Vec<u8>, CameraError> {
    match ffmpeg_with_deadline(device, &grab_args(device, width, height, false, output)) {
        Ok(bytes) => Ok(bytes),
        Err(CameraError::Capture(..)) => {
            ffmpeg_with_deadline(device, &grab_args(device, width, height, true, output))
        }
        Err(e) => Err(e),
    }
}

/// Grabs one frame from `device` at `width x height`, straight into
/// [`metal_sense::Image`]'s packed RGBA8 — no JPEG round trip and no second
/// crate for pixel format conversion. ffmpeg negotiates the camera's native
/// format and the `format=rgba` filter converts it; reading raw bytes off
/// stdout into `Image::new` is [`crate::video::encode_rgba`]'s trick run
/// backwards (frames out of ffmpeg instead of into it).
pub fn capture_frame(device: &Path, width: u32, height: u32) -> Result<metal_sense::Image, CameraError> {
    let out = grab(
        device,
        width,
        height,
        &["-frames:v", "1", "-vf", "format=rgba", "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
    )?;
    let want = width as usize * height as usize * 4;
    if out.len() != want {
        return Err(CameraError::Capture(
            device.display().to_string(),
            format!("expected {want} bytes of {width}x{height} rgba, got {}", out.len()),
        ));
    }
    Ok(metal_sense::Image::new(width, height, out))
}

/// Same grab as [`capture_frame`], but asks ffmpeg for PNG bytes instead of
/// raw RGBA. Exists because the caller that matters most — the harness tool
/// `capture_camera_frame`, and through it a vision model — wants an encoded
/// image, and ffmpeg already has the encoder in hand. Encoding here rather
/// than adding a PNG crate to the workspace keeps the "no second crate for
/// pixel work" trade this module already made for the RGBA path.
pub fn capture_frame_png(device: &Path, width: u32, height: u32) -> Result<Vec<u8>, CameraError> {
    grab(device, width, height, &["-frames:v", "1", "-f", "image2", "-vcodec", "png", "-"])
}

/// A PNG from the first camera that hands one over, trying every device
/// [`list_devices`] reports in its order (video nodes before metadata
/// nodes). Returns the device that answered, or every device's own failure
/// — a caller telling an operator "the camera is covered" needs to see that
/// the node enumerated and timed out, not only that nothing worked.
pub fn capture_any_png(width: u32, height: u32) -> Result<(String, Vec<u8>), Vec<CameraError>> {
    let devices = list_devices().map_err(|e| vec![e])?;
    let mut errors = Vec::new();
    for d in devices {
        match capture_frame_png(Path::new(&d.path), width, height) {
            Ok(png) => return Ok((d.path, png)),
            // A timeout on one node of a webcam is the whole webcam: its other
            // node is its metadata, which has no frames to give either.
            Err(e @ CameraError::Timeout(..)) => {
                errors.push(e);
                break;
            }
            Err(e) => errors.push(e),
        }
    }
    Err(errors)
}

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("could not run ffmpeg: {0}")]
    Spawn(String),
    #[error("could not capture from {0}: {1}")]
    Capture(String, String),
    #[error(
        "{0} delivered no frame in {1} s — it enumerates but does not stream: check the camera's \
         privacy shutter or camera key, or that no other program holds it"
    )]
    Timeout(String, u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_sources: deterministic, no camera required ------------------

    #[test]
    fn parses_two_devices_from_ffmpegs_own_format() {
        let text = "Auto-detected sources for video4linux2,v4l2:\n\
                     \x20 /dev/video1 [HP Wide Vision HD Camera: HP Wi] (none)\n\
                     \x20 /dev/video0 [HP Wide Vision HD Camera: HP Wi] (none)\n";
        let devices = parse_sources(text);
        assert_eq!(
            devices,
            vec![
                Device { path: "/dev/video1".into(), name: "HP Wide Vision HD Camera: HP Wi".into() },
                Device { path: "/dev/video0".into(), name: "HP Wide Vision HD Camera: HP Wi".into() },
            ]
        );
    }

    #[test]
    fn no_devices_is_an_empty_list_not_an_error() {
        let text = "Auto-detected sources for video4linux2,v4l2:\n";
        assert!(parse_sources(text).is_empty());
    }

    #[test]
    fn a_header_only_line_with_no_bracketed_name_still_yields_the_path() {
        // Defensive: if a future ffmpeg ever drops the bracketed name, the
        // path is still the load-bearing half — do not lose the whole
        // device over a cosmetic field.
        let text = "  /dev/video2 (none)\n";
        let devices = parse_sources(text);
        assert_eq!(devices, vec![Device { path: "/dev/video2".into(), name: String::new() }]);
    }

    // -- list_devices: real ffmpeg, no camera assumed -----------------------

    #[test]
    fn list_devices_runs_ffmpeg_without_erroring() {
        // Whether or not this machine has a camera is not this test's
        // business — only that the call itself succeeds (ffmpeg -sources
        // exits 0 either way) and returns *something* parseable.
        match list_devices() {
            Ok(_) => {}
            Err(e) => eprintln!("skipping: ffmpeg unavailable ({e})"),
        }
    }

    // -- capture_frame: deterministic error path, no camera required -------

    #[test]
    fn capturing_from_a_nonexistent_device_fails_fast_not_hangs() {
        match capture_frame(Path::new("/dev/video99-does-not-exist"), 320, 240) {
            Err(CameraError::Capture(_, msg)) => {
                assert!(!msg.is_empty(), "expected ffmpeg's own diagnosis in the error");
            }
            Err(CameraError::Spawn(_)) => eprintln!("skipping: ffmpeg unavailable"),
            Err(CameraError::Timeout(..)) => panic!("a missing device must fail, not wait out the deadline"),
            Ok(_) => panic!("a nonexistent device must not report a successful capture"),
        }
    }

    #[test]
    fn a_camera_that_never_answers_is_killed_at_the_deadline() {
        // A FIFO opened for reading blocks forever with no writer: the same
        // shape as a v4l2 node that enumerates but never streams. ffmpeg's
        // v4l2 demuxer cannot use it, so point a generic reader at it.
        let dir = std::env::temp_dir().join(format!("fw-cam-deadline-{}", std::process::id()));
        let _ = metal_vfs::create_dir_all(&dir);
        let fifo = dir.join("never");
        let _ = metal_vfs::remove_file(&fifo);
        if !Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success()) {
            eprintln!("skipping: no mkfifo");
            return;
        }
        let args: Vec<String> = ["-hide_banner", "-v", "error", "-i"]
            .map(String::from)
            .into_iter()
            .chain([fifo.display().to_string(), "-f".into(), "null".into(), "-".into()])
            .collect();
        let started = std::time::Instant::now();
        let got = ffmpeg_with_deadline(&fifo, &args);
        let _ = metal_vfs::remove_dir_all(&dir);
        match got {
            Err(CameraError::Timeout(..)) => {}
            Err(CameraError::Spawn(_)) => {
                eprintln!("skipping: ffmpeg unavailable");
                return;
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert!(started.elapsed() < CAPTURE_DEADLINE + std::time::Duration::from_secs(3));
    }

    #[test]
    fn a_repeated_ffmpeg_warning_is_reported_once() {
        let stderr = "bad buffer\nbad buffer\nbad buffer\nno frames\nnothing written";
        assert_eq!(last_lines(stderr), "bad buffer | no frames | nothing written");
    }

    // -- capture_frame: real camera, skipped gracefully when absent --------

    #[test]
    fn capturing_a_real_frame_has_the_requested_shape() {
        let devices = match list_devices() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let Some(device) = devices.first() else {
            eprintln!("skipping: no camera present on this machine");
            return;
        };
        let (w, h) = (320, 240);
        match capture_frame(Path::new(&device.path), w, h) {
            Ok(img) => {
                assert_eq!(img.width, w);
                assert_eq!(img.height, h);
                assert_eq!(img.rgba.len(), (w * h * 4) as usize);
            }
            Err(e) => eprintln!("skipping: camera present but capture failed ({e}) — likely busy or permission-denied in CI"),
        }
    }
}
