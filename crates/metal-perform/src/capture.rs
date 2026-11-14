//! The `capture` node's picture: a camera, a screen or one program's window,
//! live, as RGBA frames the graph treats like any other image source.
//!
//! **Shelling out, same as every other device here.** `devices` runs `pactl`,
//! `metal-listen` runs `parec`, `metal-ingest` runs ffmpeg; a capture is ffmpeg
//! decoding to raw RGBA on stdout, fed by whatever can reach the picture:
//!
//! | source id | what feeds ffmpeg |
//! |---|---|
//! | `v4l2:/dev/videoN` | the device itself — webcams, and the a6400's `Sony A6400` v4l2loopback once `launch-camera` streams into it |
//! | `dshow:<name>` | a Windows webcam through DirectShow -- `ffmpeg -f dshow -i video=<name>` |
//! | `gphoto:` | `gphoto2 --capture-movie --stdout` — a Sony in *PC Remote* mode straight off USB, no loopback module needed |
//! | `screen:<output>` | `gpu-screen-recorder -w <output>` (KMS, no picker) |
//! | `window:<niri id>` | niri's dynamic cast target set to that window, read through the portal by `gpu-screen-recorder -w portal` — the portal asks once for "Dynamic Cast Target" and the session is restored after |
//!
//! **One process per source, shared, and reaped when nobody looks.** The
//! renderer asks for a frame every tick; the first ask spawns the pipeline,
//! later asks read the newest decoded frame, and a source nobody asked for in
//! [`IDLE`] is killed — a camera light that stays on after the node is
//! deleted reads as a bug.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use web_time::{Duration, Instant};
use metal_schema::platform::Command;

use serde_json::Value;

/// The size every pipeline decodes at; the renderer's request is scaled from
/// this on the CPU, so one process serves every size the graph asks for.
pub const W: u32 = 960;
pub const H: u32 = 540;
/// A source not asked for in this long is stopped.
const IDLE: Duration = Duration::from_secs(5);
/// How long a pipeline that died waits before it is spawned again.
const RETRY: Duration = Duration::from_secs(3);

/// One thing a `capture` node can show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSource {
    pub id: String,
    pub label: String,
    /// `camera`, `sony`, `screen`, `window` or `ndi`.
    pub kind: &'static str,
}

/// Every camera, screen and window this machine offers right now. Each half
/// degrades to nothing when its tool is missing.
pub fn sources() -> Vec<VideoSource> {
    let mut out = cameras();
    if which("gphoto2") {
        out.push(VideoSource {
            id: "gphoto:".into(),
            label: "Sony A6400 · gphoto2 direct (USB, PC Remote)".into(),
            kind: "sony",
        });
    }
    out.extend(screens());
    out.extend(windows());
    // P-C: NDI streams on the network, when the runtime is installed.
    #[cfg(feature = "videoio")]
    out.extend(
        metal_videoio::capture_sources()
            .into_iter()
            .map(|(id, label)| VideoSource { id, label, kind: "ndi" }),
    );
    out
}

fn cameras() -> Vec<VideoSource> {
    if cfg!(windows) {
        return dshow_cameras();
    }
    let Ok(dir) = metal_vfs::read_dir("/sys/class/video4linux") else {
        return Vec::new();
    };
    let mut list: Vec<(u32, String)> = dir
        .flatten()
        .filter_map(|e| {
            let node = e.file_name().to_string_lossy().to_string();
            let n: u32 = node.strip_prefix("video")?.parse().ok()?;
            let name = metal_vfs::read_to_string(e.path().join("name")).unwrap_or_default();
            Some((n, name.trim().to_string()))
        })
        .collect();
    list.sort();
    list.into_iter()
        .map(|(n, name)| {
            let sony = name.contains("Sony") || name.contains("A6400");
            VideoSource {
                id: format!("v4l2:/dev/video{n}"),
                label: format!("{} · /dev/video{n}", if name.is_empty() { "camera" } else { &name }),
                kind: if sony { "sony" } else { "camera" },
            }
        })
        .collect()
}

fn dshow_cameras() -> Vec<VideoSource> {
    // `ffmpeg -list_devices` writes its listing to stderr and exits non-zero,
    // so `run` (stdout + success) cannot read it. Only the inline
    // `"name" (video)` lines are cameras; `(audio)` is a microphone.
    let Ok(out) = Command::new("ffmpeg")
        .args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .output()
    else {
        return Vec::new();
    };
    parse_dshow_devices(&String::from_utf8_lossy(&out.stderr))
}

fn parse_dshow_devices(text: &str) -> Vec<VideoSource> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(tail) = line.split_once('"').map(|(_, t)| t) else { continue };
        let Some((name, rest)) = tail.rsplit_once('"') else { continue };
        if name.is_empty() || !rest.trim_start().starts_with("(video)") {
            continue;
        }
        if out.iter().any(|s: &VideoSource| s.label.starts_with(name)) {
            continue;
        }
        out.push(VideoSource {
            id: format!("dshow:{name}"),
            label: format!("{name} -- DirectShow"),
            kind: "camera",
        });
    }
    out
}

fn screens() -> Vec<VideoSource> {
    if let Some(text) = run("niri", &["msg", "-j", "outputs"]) {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
            let mut names: Vec<&String> = map.keys().collect();
            names.sort();
            return names
                .into_iter()
                .map(|n| VideoSource { id: format!("screen:{n}"), label: format!("screen · {n}"), kind: "screen" })
                .collect();
        }
    }
    crate::devices::displays()
        .into_iter()
        .map(|d| VideoSource { id: format!("screen:{}", d.id), label: format!("screen · {}", d.label), kind: "screen" })
        .collect()
}

fn windows() -> Vec<VideoSource> {
    let Some(text) = run("niri", &["msg", "-j", "windows"]) else {
        return Vec::new();
    };
    parse_niri_windows(&text)
}

fn parse_niri_windows(text: &str) -> Vec<VideoSource> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|w| {
            let id = w["id"].as_u64()?;
            let app = w["app_id"].as_str().unwrap_or("window");
            let title = w["title"].as_str().unwrap_or("");
            Some(VideoSource { id: format!("window:{id}"), label: format!("window · {app} · {title}"), kind: "window" })
        })
        .collect()
}

/// The shell pipeline for `id`, or `None` for an id that does not parse —
/// every value spliced in is checked to be a device path, an output name or a
/// number first, since a node's config is user data.
fn pipeline(id: &str) -> Option<String> {
    let decode = format!(
        "ffmpeg -hide_banner -loglevel error -fflags nobuffer -flags low_delay {{IN}} \
         -vf scale={W}:{H}:force_original_aspect_ratio=decrease,pad={W}:{H}:(ow-iw)/2:(oh-ih)/2 \
         -pix_fmt rgba -f rawvideo -"
    );
    let safe = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c));
    if let Some(dev) = id.strip_prefix("v4l2:") {
        (dev.starts_with("/dev/video") && safe(dev)).then(|| decode.replace("{IN}", &format!("-f v4l2 -i {dev}")))
    } else if let Some(name) = id.strip_prefix("dshow:") {
        // A Windows webcam: `dshow:<device name>`, the caption ffmpeg's
        // `-list_devices` prints (e.g. `dshow:HD Pro Webcam C920`). A device
        // description may carry spaces and punctuation but never a shell
        // metacharacter; it is quoted for ffmpeg.
        let ok = !name.is_empty()
            && !name.starts_with('-')
            && name.chars().all(|c| c.is_alphanumeric() || " -_.,()+/:".contains(c))
            && name.chars().any(|c| c.is_alphanumeric());
        ok.then(|| decode.replace("{IN}", &format!("-f dshow -rtbufsize 100M -i \"video={name}\"")))
    } else if id == "gphoto:" {
        Some(format!("gphoto2 --capture-movie --stdout 2>/dev/null | {}", decode.replace("{IN}", "-f mjpeg -i -")))
    } else if let Some(out) = id.strip_prefix("screen:") {
        safe(out).then(|| {
            format!(
                "gpu-screen-recorder -w {out} -c mpegts -f 30 -cursor no -o /dev/stdout 2>/dev/null | {}",
                decode.replace("{IN}", "-f mpegts -i -")
            )
        })
    } else if let Some(win) = id.strip_prefix("window:") {
        win.chars().all(|c| c.is_ascii_digit()).then(|| {
            format!(
                "niri msg action set-dynamic-cast-window --id {win} >/dev/null 2>&1; \
                 gpu-screen-recorder -w portal -restore-portal-session yes -c mpegts -f 30 -o /dev/stdout 2>/dev/null | {}",
                decode.replace("{IN}", "-f mpegts -i -")
            )
        })
    } else {
        None
    }
}

/// The newest decoded frame and its sequence number (a new number per frame,
/// so a consumer can tell "same frame" without comparing pixels).
type Latest = Arc<Mutex<Option<(u64, Arc<Vec<u8>>)>>>;

struct Grab {
    child: Option<Child>,
    latest: Latest,
    alive: Arc<Mutex<bool>>,
    asked: Instant,
    spawned: Instant,
}

impl Grab {
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // The pipeline is a process group (`process_group(0)` below):
            // killing the shell alone would leave gphoto2/ffmpeg holding the
            // camera.
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Grab {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn(cmd: &str) -> Option<(Child, Latest, Arc<Mutex<bool>>)> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let latest = Arc::new(Mutex::new(None));
    let alive = Arc::new(Mutex::new(true));
    let (l, a) = (Arc::clone(&latest), Arc::clone(&alive));
    std::thread::spawn(move || {
        let mut buf = vec![0u8; (W * H * 4) as usize];
        let mut seq = 0u64;
        while stdout.read_exact(&mut buf).is_ok() {
            seq += 1;
            *l.lock().expect("capture frame") = Some((seq, Arc::new(buf.clone())));
        }
        *a.lock().expect("capture alive") = false;
    });
    Some((child, latest, alive))
}

fn registry() -> &'static Mutex<HashMap<String, Grab>> {
    static REG: OnceLock<Mutex<HashMap<String, Grab>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The newest frame of `source`, scaled to `w × h`; `None` until the first
/// frame lands, or for a source that cannot be opened. Starts the pipeline
/// on first ask and stops every other source left idle.
pub fn frame(source: &str, w: u32, h: u32) -> Option<Vec<u8>> {
    // P-C: `ndi:<name>` is received by `metal-videoio`, not an ffmpeg pipe.
    #[cfg(feature = "videoio")]
    if source.starts_with("ndi:") {
        let (px, sw, sh) = metal_videoio::capture_frame(source)?;
        return Some(scale(&px, sw, sh, w, h));
    }
    let (_, latest) = latest(source)?;
    Some(scale(&latest, W, H, w, h))
}

/// The newest frame of `source` at its native `W x H`, with its sequence
/// number (G2: the GPU backend uploads it once per new frame and lets compose
/// scale it). `None` as [`frame`], and for `ndi:` sources (the CPU path).
pub fn latest_raw(source: &str) -> Option<(u64, Arc<Vec<u8>>)> {
    #[cfg(feature = "videoio")]
    if source.starts_with("ndi:") {
        return None;
    }
    latest(source)
}

fn latest(source: &str) -> Option<(u64, Arc<Vec<u8>>)> {
    let cmd = pipeline(source)?;
    let mut reg = registry().lock().ok()?;
    let now = Instant::now();
    reg.retain(|id, g| id == source || now.duration_since(g.asked) < IDLE);
    let grab = reg.entry(source.to_string()).or_insert_with(|| Grab {
        child: None,
        latest: Arc::new(Mutex::new(None)),
        alive: Arc::new(Mutex::new(false)),
        asked: now,
        spawned: now - RETRY,
    });
    grab.asked = now;
    let dead = !*grab.alive.lock().ok()?;
    if dead && now.duration_since(grab.spawned) >= RETRY {
        grab.stop();
        grab.spawned = now;
        if let Some((child, latest, alive)) = spawn(&cmd) {
            grab.child = Some(child);
            grab.latest = latest;
            grab.alive = alive;
        }
    }
    let got = grab.latest.lock().ok()?.clone();
    got
}

/// Nearest-neighbour RGBA scale — a live picture at preview sizes, where a
/// filter would cost more than it shows.
pub fn scale(src: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Vec<u8> {
    if (sw, sh) == (w, h) {
        return src.to_vec();
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let sy = (y as u64 * sh as u64 / h.max(1) as u64) as u32;
        for x in 0..w {
            let sx = (x as u64 * sw as u64 / w.max(1) as u64) as u32;
            let s = ((sy * sw + sx) * 4) as usize;
            let d = ((y * w + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| metal_vfs::is_file(&d.join(program))))
        .unwrap_or(false)
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipelines_only_splice_checked_values() {
        assert!(pipeline("v4l2:/dev/video2").unwrap().contains("-f v4l2 -i /dev/video2"));
        assert!(pipeline("v4l2:/dev/video2; rm -rf /").is_none());
        assert!(pipeline("v4l2:/etc/passwd").is_none());
        assert!(pipeline("screen:HDMI-A-2").unwrap().contains("-w HDMI-A-2"));
        assert!(pipeline("screen:$(id)").is_none());
        assert!(pipeline("window:16").unwrap().contains("--id 16"));
        assert!(pipeline("window:1 2").is_none());
        assert!(pipeline("gphoto:").unwrap().starts_with("gphoto2 --capture-movie"));
        assert!(pipeline("nonsense").is_none());
        // A Windows webcam's caption carries spaces and is quoted.
        assert!(pipeline("dshow:HD Pro Webcam C920").unwrap().contains("-i \"video=HD Pro Webcam C920\""));
        assert!(pipeline("dshow:Logitech BRIO (4K)").is_some());
        // Shell metacharacters are refused outright.
        assert!(pipeline("dshow:").is_none());
        assert!(pipeline("dshow:cam && calc").is_none());
        assert!(pipeline("dshow:cam; rm -rf /").is_none());
        assert!(pipeline("dshow:$(id)").is_none());
        assert!(pipeline("dshow:-bad").is_none());
        assert!(frame("nonsense", 4, 4).is_none());
    }

    #[test]
    fn niri_windows_are_listed_by_id() {
        let text = r#"[{"id":16,"title":"Home","app_id":"org.gnome.Nautilus"},{"title":"no id"}]"#;
        assert_eq!(
            parse_niri_windows(text),
            vec![VideoSource { id: "window:16".into(), label: "window · org.gnome.Nautilus · Home".into(), kind: "window" }]
        );
    }

    #[test]
    #[test]
    fn dshow_listing_reads_only_video_captions() {
        let text = "[dshow @ 002] \"HD Pro Webcam C920\" (video)\r\n\
                    [dshow @ 002]   Alternative name \"@device_pnp_x\"\r\n\
                    [dshow @ 002] \"Microphone (USB Audio)\" (audio)\r\n\
                    [dshow @ 002] \"Logitech BRIO\" (video)\r\n";
        let cams = parse_dshow_devices(text);
        assert_eq!(cams.len(), 2, "{cams:?}");
        assert_eq!(cams[0].id, "dshow:HD Pro Webcam C920");
        assert!(pipeline(&cams[0].id).is_some());
        assert!(parse_dshow_devices("no devices").is_empty());
    }

    #[test]
    fn scale_picks_the_nearest_pixel() {
        let src = [1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4];
        assert_eq!(scale(&src, 2, 2, 1, 1), vec![1, 1, 1, 1]);
        assert_eq!(scale(&src, 2, 2, 4, 2).len(), 32);
    }
}
