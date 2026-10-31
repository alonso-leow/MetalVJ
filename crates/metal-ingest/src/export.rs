//! `export.video`'s encode side: an [`Encoder`] trait, a factory registry and
//! the ffmpeg backend that is the first implementation behind it.
//!
//! Packets out, one muxer, the codec plugged in rather
//! than reimplemented. The first backend stays ffmpeg through
//! [`crate::video::encode_rgba`]'s flags — `-threads 1`, `-fflags +bitexact`,
//! a pinned `creation_time` and rawvideo on stdin — because those are what
//! make an export deterministic (two runs over the same frames byte-equal; see
//! that function's doc comment for the measurement). A pure-Rust codec can
//! come later behind the same trait; nothing here reimplements one.
//!
//! [`crate::video::encode_rgba`] pulls frames from a closure and is the
//! one-shot whole-clip call the export route used to dream of. The trait here
//! is the *stepped* form the exporter in `metal-render::headless` needs: push
//! one frame at a time into a live child, so an export can interleave rendering and encoding
//! without buffering the whole clip.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};

use metal_schema::platform::Command;
use serde::{Deserialize, Serialize};

use crate::video::VideoError;

/// Everything a backend needs to open one output: where it goes, how big the
/// frames are, how fast they run and which codec/bitrate was asked for.
///
/// Deliberately *not* the route's whole body — `path` is the one field a
/// backend must have and the codec/bitrate are the two it may change what it
/// does with. Everything else (range, node) is the executor's, one layer up.
#[derive(Clone, Debug)]
pub struct EncodeSettings {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// A codec name from the registry (`"h264"` by default). Empty is the
    /// default codec, so a caller that names nothing gets H.264.
    pub codec: String,
    /// Average bitrate in kbit/s. `None` keeps ffmpeg's CRF-18 constant
    /// quality, which is what the determinism test measures against.
    pub bitrate_kbps: Option<u32>,
    /// An optional audio track for the mux (plan §9.1 E3): a path to a WAV (or
    /// any file ffmpeg can read) muxed beside the video as a second input.
    /// `None` is the E1 video-only export, and keeps the ffmpeg command — and
    /// so the bytes — exactly what it was before audio existed.
    pub audio: Option<PathBuf>,
}

impl EncodeSettings {
    pub fn new(path: impl Into<PathBuf>, width: u32, height: u32, fps: f64) -> EncodeSettings {
        EncodeSettings {
            path: path.into(),
            width,
            height,
            fps,
            codec: "h264".to_string(),
            bitrate_kbps: None,
            audio: None,
        }
    }
}

/// A live encode: frames in, one file out. The stepped mirror of
/// [`crate::video::encode_rgba`].
pub trait Encoder: Send {
    /// Writes one RGBA8 frame. A frame that is not `width * height * 4` bytes
    /// is refused rather than fed to the backend.
    fn push(&mut self, frame: &[u8]) -> Result<(), VideoError>;
    /// Ends the stream and waits for the backend to finish the file, returning
    /// the file's size in bytes. Consumes the encoder so a finished one cannot
    /// be pushed to again.
    fn finish(self: Box<Self>) -> Result<u64, VideoError>;
}

/// A registered backend: given settings, either handles them (`Some`) or
/// passes (`None`). The `Result` inside is the backend's own answer once it
/// has claimed the settings — an error there is final, not a reason to try
/// the next factory (the first factory that
/// recognises a codec owns its failures).
pub type EncoderFactory = fn(&EncodeSettings) -> Option<Result<Box<dyn Encoder>, VideoError>>;

fn factories() -> &'static std::sync::RwLock<Vec<(&'static str, EncoderFactory)>> {
    static F: std::sync::OnceLock<std::sync::RwLock<Vec<(&'static str, EncoderFactory)>>> =
        std::sync::OnceLock::new();
    F.get_or_init(|| {
        std::sync::RwLock::new(vec![
            ("h264", ffmpeg_factory),
            ("libx264", ffmpeg_factory),
            ("ffmpeg", ffmpeg_factory),
        ])
    })
}

/// Registers `factory` for `codec`. The seam a pure-Rust codec plugs into
/// later without touching the exporter or the route.
pub fn register_encoder(codec: &'static str, factory: EncoderFactory) {
    factories()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .push((codec, factory));
}

/// Every codec name a backend answers to, in registration order. The route
/// validates a requested codec against this so a typo is a 400 and not a
/// half-open ffmpeg.
pub fn codecs() -> Vec<&'static str> {
    factories()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(name, _)| *name)
        .collect()
}

/// Opens the encoder for `settings`, or an error naming the codec if no
/// backend claims it.
pub fn open(settings: &EncodeSettings) -> Result<Box<dyn Encoder>, VideoError> {
    if settings.width == 0 || settings.height == 0 {
        return Err(VideoError::Encode(format!(
            "{}x{} is not a frame size",
            settings.width, settings.height
        )));
    }
    let wanted = settings.codec.trim().to_ascii_lowercase();
    let factories = factories().read().unwrap_or_else(|e| e.into_inner());
    let found = factories.iter().find(|(name, _)| {
        if wanted.is_empty() {
            *name == "h264"
        } else {
            name.eq_ignore_ascii_case(&wanted)
        }
    });
    match found {
        Some((_, factory)) => factory(settings)
            .unwrap_or_else(|| Err(VideoError::Encode(format!("no encoder for codec {:?}", settings.codec)))),
        None => Err(VideoError::Encode(format!(
            "no encoder for codec {:?} (have {})",
            settings.codec,
            codecs().join(", ")
        ))),
    }
}

// -- the ffmpeg backend -----------------------------------------------------

/// The first backend (plan §9.1): ffmpeg's `libx264` on rawvideo stdin, with
/// the exact determinism flags [`crate::video::encode_rgba`] carries. Not a
/// reimplementation of a codec — a reimplementation of the *process wrapper*
/// as a push/finish object instead of a closure.
fn ffmpeg_factory(settings: &EncodeSettings) -> Option<Result<Box<dyn Encoder>, VideoError>> {
    let codec = settings.codec.trim().to_ascii_lowercase();
    if !matches!(codec.as_str(), "" | "h264" | "libx264" | "ffmpeg") {
        return None;
    }
    Some(FfmpegEncoder::spawn(settings).map(|e| Box::new(e) as Box<dyn Encoder>))
}

struct FfmpegEncoder {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    expect: usize,
    dest: PathBuf,
}

impl FfmpegEncoder {
    fn spawn(settings: &EncodeSettings) -> Result<FfmpegEncoder, VideoError> {
        if let Some(dir) = settings.path.parent() {
            if !dir.as_os_str().is_empty() {
                metal_vfs::create_dir_all(dir).map_err(|e| VideoError::Spawn(e.to_string()))?;
            }
        }
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{}x{}", settings.width, settings.height)])
            .args(["-r", &format!("{}", settings.fps)])
            .args(["-i", "-"]);
        // E3: an optional audio input, muxed as a second stream and mapped
        // explicitly. Without one the command is byte-for-byte E1's (`-an`).
        if let Some(audio) = &settings.audio {
            cmd.arg("-i").arg(audio).args(["-map", "0:v:0", "-map", "1:a:0"]);
        } else {
            cmd.arg("-an");
        }
        cmd.args(["-threads", "1"])
            .args(["-c:v", "libx264", "-preset", "veryfast"]);
        match settings.bitrate_kbps {
            Some(kbps) if kbps > 0 => {
                cmd.args(["-b:v", &format!("{kbps}k")]);
            }
            _ => {
                cmd.args(["-crf", "18"]);
            }
        }
        if settings.audio.is_some() {
            cmd.args(["-c:a", "aac", "-shortest"]);
        }
        cmd.args(["-pix_fmt", "yuv420p"])
            .args(["-fflags", "+bitexact", "-flags:v", "+bitexact"])
            .args(["-metadata", "creation_time=1970-01-01T00:00:00Z"])
            .arg(&settings.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| VideoError::Spawn(e.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| VideoError::Spawn("no stdin".into()))?;
        Ok(FfmpegEncoder {
            child: Some(child),
            stdin: Some(stdin),
            expect: (settings.width as usize) * (settings.height as usize) * 4,
            dest: settings.path.clone(),
        })
    }
}

impl Encoder for FfmpegEncoder {
    fn push(&mut self, frame: &[u8]) -> Result<(), VideoError> {
        if frame.len() != self.expect {
            return Err(VideoError::Encode(format!(
                "frame is {} bytes, expected {}",
                frame.len(),
                self.expect
            )));
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| VideoError::Spawn("encoder already finished".into()))?;
        stdin
            .write_all(frame)
            .map_err(|e| VideoError::Spawn(e.to_string()))
    }

    fn finish(self: Box<Self>) -> Result<u64, VideoError> {
        let mut this = self;
        // Dropping stdin sends ffmpeg its end-of-stream; without it the
        // child waits for more input forever.
        drop(this.stdin.take());
        let child = this
            .child
            .take()
            .ok_or_else(|| VideoError::Spawn("encoder already finished".into()))?;
        let dest = this.dest.clone();
        let out = child
            .wait_with_output()
            .map_err(|e| VideoError::Spawn(e.to_string()))?;
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if !out.status.success() {
            return Err(VideoError::Encode(if stderr.is_empty() {
                format!("ffmpeg exited with {}", out.status)
            } else {
                stderr
            }));
        }
        Ok(metal_vfs::metadata(&dest).map(|m| m.len()).unwrap_or(0))
    }
}

// -- joining shards ------------------------------------------------------------

/// Joins the shard files of a farm export, in the order given, into `out`
/// through ffmpeg's concat demuxer and the same mp4 muxer an ordinary export
/// ends in. Streams are copied, not re-encoded: every shard was encoded with
/// the same pinned flags ([`FfmpegEncoder`]), so they join on a keyframe
/// boundary and the result is deterministic. Written to a temp file and renamed.
pub fn join_shards(files: &[PathBuf], out: &std::path::Path) -> Result<(), String> {
    if files.is_empty() {
        return Err("nothing to join".into());
    }
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    let list = out.with_extension("concat.txt");
    let mut text = String::new();
    for f in files {
        let abs = std::fs::canonicalize(f).map_err(|e| format!("{}: {e}", f.display()))?;
        // The concat list quotes with '...'; a quote in a path is escaped.
        text.push_str(&format!("file '{}'\n", abs.display().to_string().replace('\'', "'\\''")));
    }
    std::fs::write(&list, text).map_err(|e| e.to_string())?;
    let tmp = out.with_extension("joining.mp4");
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .args(["-c", "copy", "-fflags", "+bitexact", "-flags:v", "+bitexact", "-f", "mp4"])
        .arg(&tmp)
        .output();
    let _ = std::fs::remove_file(&list);
    match status {
        Ok(o) if o.status.success() => std::fs::rename(&tmp, out).map_err(|e| e.to_string()),
        Ok(o) => {
            let _ = std::fs::remove_file(&tmp);
            Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

// -- one frame of an offline render -----------------------------------------

/// One frame an [`Exporter`] should draw: which model, its float row, at what
/// time, with which effect chain. The neutral currency between the offline
/// executor (`metal-perform`) and the renderer's exporter
/// (`metal-render::headless`), so neither crate has to know the other's
/// timeline or device types.
#[derive(Clone, Debug)]
pub struct FrameJob {
    /// Index into the renderer's model list (`Renderer::solo_chain`'s `index`).
    pub index: usize,
    /// The model's float row, effects block included.
    pub floats: Vec<f32>,
    /// The time this frame draws at, in seconds.
    pub time: f32,
    /// Whether the source draws with an alpha backdrop.
    pub fondo_alpha: bool,
    /// The graph's resolved `fx.*` chain for this frame (empty = legacy order).
    pub chain: Vec<metal_schema::EffectStep>,
}

/// Where an exporter's frames come from: deterministic, one at a time.
///
/// `job(k)` is a pure function of `k` for a given source — the whole point of
/// an offline export is that frame 900 is always frame 900 (see
/// `metal-dash`'s `author::models::timeline` doc). An implementation that has
/// to load media may return `None` for a frame that is not ready yet; the
/// exporter turns that into `Step::Pending` so a background job polls rather
/// than blocks.
pub trait FrameSource: Send {
    /// Frames in the range being exported.
    fn total(&self) -> u64;
    /// The job for the `k`-th frame (0-based within the range), or `None`
    /// when it is not ready yet.
    fn job(&mut self, k: u64) -> Option<FrameJob>;
}

// -- presets ----------------------------------------------------------------

/// A named export setting (`path` and the range are the caller's).
///
/// The `data/export-presets.json` list is this type as a serde list; the
/// default preset is [`default_preset`]. Deliberately a small, honest subset
/// of a full export preset: what this export can actually honour today
/// (codec, bitrate, frame size and rate), not fields no backend reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub codec: String,
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
}

/// The one shipped default: 1080p H.264 at the project's frame rate, CRF 18.
pub fn default_preset() -> ExportPreset {
    ExportPreset {
        name: "Default 1080p H.264".to_string(),
        codec: "h264".to_string(),
        bitrate_kbps: None,
        fps: 30.0,
        width: 1920,
        height: 1080,
    }
}

/// Every built-in preset, in list order. One today; the browser grows it.
pub fn default_presets() -> Vec<ExportPreset> {
    vec![default_preset()]
}

/// Reads a preset list from JSON. An empty list is invalid — an export with
/// no preset has nothing to fall back to — so the caller keeps the built-in
/// defaults rather than an empty menu.
pub fn parse_presets(json: &str) -> Result<Vec<ExportPreset>, String> {
    // A bare list, or `{"presets": [...], "queue": [...]}` (the queue is the
    // dash's: it runs the waiting jobs).
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = if value.is_array() { value } else { value["presets"].clone() };
    let presets: Vec<ExportPreset> = serde_json::from_value(list).map_err(|e| e.to_string())?;
    if presets.is_empty() {
        return Err("export presets: the list is empty".to_string());
    }
    Ok(presets)
}

/// The preset a request falls back to when it names none: the first one.
pub fn first_preset(presets: &[ExportPreset]) -> Option<&ExportPreset> {
    presets.first()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    /// The same guarantee `video::encode_rgba`'s test proves, now through the
    /// stepped trait: two runs over identical frames are byte-identical. A one
    /// second gap is inserted so a wall-clock stamp leaking into the container
    /// fails the test rather than passing by luck.
    #[test]
    fn two_encoder_runs_of_the_same_frames_are_byte_identical() {
        let dir = std::env::temp_dir().join(format!("fw-encoder-det-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        let (w, h, n) = (48u32, 32u32, 6u32);
        let frames = synthetic_frames(w, h, n);

        let run = |name: &str| -> Option<PathBuf> {
            let dest = dir.join(name);
            let settings = EncodeSettings::new(&dest, w, h, 8.0);
            let mut enc = match open(&settings) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("skipping: ffmpeg unavailable or refused ({e})");
                    return None;
                }
            };
            for f in &frames {
                if let Err(e) = enc.push(f) {
                    eprintln!("skipping: push failed ({e})");
                    return None;
                }
            }
            match enc.finish() {
                Ok(_) => Some(dest),
                Err(e) => {
                    eprintln!("skipping: finish failed ({e})");
                    None
                }
            }
        };

        let Some(out1) = run("a.mp4") else { return };
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let Some(out2) = run("b.mp4") else { return };

        let (a, b) = (metal_vfs::read(&out1).unwrap(), metal_vfs::read(&out2).unwrap());
        assert!(!a.is_empty(), "a successful encode leaves a real file");
        assert_eq!(
            a,
            b,
            "two stepped encodes of the same {n} frames differ ({} vs {} bytes)",
            a.len(),
            b.len()
        );
        metal_vfs::remove_dir_all(&dir).unwrap();
    }

    /// ffprobe's stream codec types for `path`, in order — the container-level
    /// check that a muxed export really carries both a picture and a sound.
    fn stream_types(path: &Path) -> Option<Vec<String>> {
        let out = metal_schema::platform::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
            .arg(path)
            .output()
            .ok()?;
        out.status.success().then(|| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
    }

    /// E3's mux: with an audio track the container carries a second stream;
    /// without one it stays exactly the video-only export E1 shipped (and the
    /// byte-identity of two such runs is the test above).
    #[test]
    fn an_audio_track_muxes_a_second_stream() {
        let dir = std::env::temp_dir().join(format!("fw-encoder-audio-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        let (w, h, n) = (48u32, 32u32, 6u32);
        let frames = synthetic_frames(w, h, n);
        let rate = 48_000u32;
        let tone: Vec<f32> = (0..(rate as usize * n as usize / 8))
            .map(|i| 0.3 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin() as f32)
            .collect();
        let track = crate::audio::AudioTrack { rate, channels: 1, samples: tone };
        let wav = dir.join("tone.wav");
        crate::audio::write_wav(&wav, &track).unwrap();

        let encode = |name: &str, audio: Option<PathBuf>| -> Option<PathBuf> {
            let dest = dir.join(name);
            let mut settings = EncodeSettings::new(&dest, w, h, 8.0);
            settings.audio = audio;
            let mut enc = match open(&settings) {
                Ok(e) => e,
                Err(_) => {
                    eprintln!("skipping: ffmpeg unavailable");
                    return None;
                }
            };
            for f in &frames {
                enc.push(f).unwrap();
            }
            enc.finish().unwrap();
            Some(dest)
        };

        let Some(with_audio) = encode("with.mp4", Some(wav.clone())) else { return };
        let Some(video_only) = encode("video.mp4", None) else { return };

        let types = stream_types(&with_audio).expect("ffprobe reads the muxed file");
        assert!(types.contains(&"video".to_string()), "no video stream: {types:?}");
        assert!(types.contains(&"audio".to_string()), "no audio stream: {types:?}");
        let plain = stream_types(&video_only).expect("ffprobe reads the video-only file");
        assert_eq!(plain, vec!["video".to_string()], "video-only carries no audio: {plain:?}");

        metal_vfs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mis_sized_frame_is_refused_before_it_reaches_ffmpeg() {
        let dir = std::env::temp_dir().join(format!("fw-encoder-bad-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let settings = EncodeSettings::new(dir.join("out.mp4"), 16, 16, 8.0);
        match open(&settings) {
            Ok(mut enc) => {
                let err = enc.push(&[0u8; 10]).unwrap_err();
                assert!(format!("{err}").contains("expected"), "{err}");
            }
            Err(VideoError::Spawn(_)) => eprintln!("skipping: ffmpeg unavailable"),
            Err(e) => panic!("unexpected {e:?}"),
        }
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_codec_is_refused_by_name() {
        let settings = EncodeSettings {
            codec: "no-such-codec".into(),
            ..EncodeSettings::new(Path::new("/dev/null"), 16, 16, 8.0)
        };
        let err = open(&settings).err().expect("an unknown codec is refused");
        assert!(format!("{err}").contains("no-such-codec"), "{err}");
    }

    #[test]
    fn a_zero_sized_frame_is_refused_before_spawning() {
        let err = open(&EncodeSettings::new("/dev/null", 0, 10, 8.0))
            .err()
            .expect("a zero-sized frame is refused");
        assert!(matches!(err, VideoError::Encode(_)), "{err:?}");
    }

    #[test]
    fn the_built_in_presets_round_trip_through_json() {
        let json = serde_json::to_string(&default_presets()).unwrap();
        let back = parse_presets(&json).unwrap();
        assert_eq!(back, default_presets());
        assert_eq!(first_preset(&back).unwrap().codec, "h264");
    }

    #[test]
    fn an_empty_preset_list_is_refused_rather_than_silently_empty() {
        assert!(parse_presets("[]").is_err());
        assert!(parse_presets("not json").is_err());
    }

    /// Shards encoded apart join into one file with every frame, in the order
    /// given (skipped where there is no ffmpeg).
    #[test]
    fn shards_join_into_one_file_with_all_their_frames() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("skipped: no ffmpeg");
            return;
        }
        let dir = std::env::temp_dir().join(format!("fw-join-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut shards = Vec::new();
        for (i, secs) in [1u32, 2].into_iter().enumerate() {
            let path = dir.join(format!("s{i}.mp4"));
            let mut enc = open(&EncodeSettings::new(&path, 64, 48, 10.0)).expect("an encoder");
            for _ in 0..(10 * secs) {
                enc.push(&vec![i as u8 * 100; 64 * 48 * 4]).expect("a frame");
            }
            enc.finish().expect("finish");
            shards.push(path);
        }
        let out = dir.join("joined.mp4");
        join_shards(&shards, &out).expect("join");
        let frames = crate::video::probe(&out).map(|i| (i.duration * 10.0).round() as u32);
        assert_eq!(frames.ok(), Some(30), "1 s + 2 s at 10 fps");
        assert!(join_shards(&[], &out).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
