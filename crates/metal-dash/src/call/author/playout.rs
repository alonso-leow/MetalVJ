//! 2026-10-01: what the sequence editor plays and keeps.
//!
//! - `POST /author/play {song, from}` plays the song's audio from `from`
//!   seconds (`ffplay`, no window), replacing whatever was playing;
//!   `{stop: true}` (or no song) stops it. The editor posts it when its
//!   playback starts and stops, so the picture and the sound start together.
//! - `POST /author/video/seek {path, t}` and `GET /author/video/frame.jpg?path=`
//!   — a video clip's picture. The first seek of a file makes its HAP proxy
//!   (`data/projects/hap/<slug>.mov` + `.json`, at the digest's 8 frames a
//!   second, 480 px wide) once, kept for every later session; the frame route
//!   reads the frame nearest the last seek straight from it (the preview's
//!   query stays put while the clip plays, so its picture never blanks). If
//!   the proxy cannot be made, one `ffmpeg` decodes the clip to MJPEG in
//!   memory instead, as before; on Android (no ffmpeg) AMediaCodec decodes it
//!   instead (`decode_native`). The same pass cuts the clip into scenes
//!   (`<slug>.scenes.json` + posters; see `super::scenes`).
//! - `GET /author/project?song=` / `POST /author/project` — the sequence as
//!   one file per song (`data/projects/<song>.json`): every page (a device,
//!   or a mask of it), every layer, every clip with its marks and keypoints.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use metal_schema::platform::Command;

use serde_json::{json, Value};

use crate::call::{Bytes, CallError, Params};

use super::Ctx;

/// Where the projects live, one JSON per song.
pub const PROJECTS: &str = "data/projects";
/// The width a video clip is decoded at — a preview, not the output.
const VIDEO_W: u32 = 480;
/// Decoded frames per second — the digest's own tick rate.
const VIDEO_FPS: f32 = crate::digest::RATE;

fn player() -> &'static Mutex<Option<Child>> {
    static P: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// The desktop ffplay run's clock: when its child was spawned, and the song
/// second it started from. ffplay has no clock the process can read, so the
/// position is the wall clock's from that spawn, for as long as the child runs
/// (a stop kills the child and clears this; a finished run stops answering).
#[cfg(not(any(target_os = "android", feature = "native-audio")))]
fn ffplay_wall() -> &'static Mutex<Option<(Instant, f64)>> {
    static W: OnceLock<Mutex<Option<(Instant, f64)>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(None))
}

/// Whether the ffplay child is still running, and where the wall clock says
/// the song is. `None` once it has exited or was never started.
#[cfg(not(any(target_os = "android", feature = "native-audio")))]
fn ffplay_clock() -> Option<f64> {
    let mut slot = player().lock().expect("player lock");
    let running = matches!(slot.as_mut()?.try_wait(), Ok(None));
    let (started, from) = (*ffplay_wall().lock().expect("wall lock"))?;
    running.then(|| from + started.elapsed().as_secs_f64())
}

/// The song's audio file in [`crate::author::SONGS`]: the file whose stem is
/// `song`, with a sound extension.
pub(crate) fn song_audio(song: &str) -> Option<PathBuf> {
    metal_vfs::read_dir(crate::author::SONGS).ok()?.flatten().map(|e| e.path()).find(|p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_lowercase();
        p.file_stem().and_then(|s| s.to_str()) == Some(song)
            && crate::author::audio::SOUND.iter().any(|ext| name.ends_with(ext))
    })
}

/// The musical clock's side of playback (`POST /loops/song`, the engine's
/// `State::play_song`) lives in `crate::call::loops::song`; these helpers
/// find and read the beat map it follows.
///
/// The song's tempo sidecar: `<stem>.tempo.json` beside its audio — where
/// ingest's `digest_to` writes it (next to `<stem>.8hz.npy`).
pub(crate) fn sidecar(audio: &Path) -> PathBuf {
    let stem = audio.file_stem().and_then(|s| s.to_str()).unwrap_or("song");
    metal_listen::file::tempo_path(&audio.with_file_name(metal_schema::digest_file_name(stem, metal_schema::TickRate::Base)))
}

/// The beat map the musical clock follows while `audio` plays (the loops design notes
/// T15), read from its sidecar. `None` when there is none or it does not
/// parse: the clock then stays on the live input (or its free clock).
pub(crate) fn tempo_track(audio: &Path) -> Option<metal_perform::clock::TempoTrack> {
    let text = metal_vfs::read_to_string(sidecar(audio)).ok()?;
    let map = metal_listen::file::TempoMap::from_json(&text).ok()?;
    Some(metal_perform::clock::TempoTrack { segments: map.segments, beats: map.beats, downbeats: map.downbeats })
}

/// A song with no sidecar (ingested before T3, or dropped in by hand) gets
/// one written in the background so its *next* play follows its beats; this
/// play keeps the live/free clock. Not in tests: it decodes the whole song.
pub(crate) fn make_sidecar_later(audio: &Path) {
    if cfg!(test) {
        return;
    }
    static BUSY: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    let busy = BUSY.get_or_init(Default::default);
    if !busy.lock().expect("sidecar lock").insert(audio.to_path_buf()) {
        return;
    }
    let audio = audio.to_path_buf();
    std::thread::spawn(move || {
        if let Ok((_, map)) = metal_listen::file::digest_file_with_tempo(&audio) {
            let _ = metal_vfs::write(sidecar(&audio), map.to_json());
        }
        busy.lock().expect("sidecar lock").remove(&audio);
    });
}

pub fn play(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().unwrap_or_else(|| json!({}));
    let mut slot = player().lock().expect("player lock");
    if let Some(mut child) = slot.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(any(target_os = "android", feature = "native-audio"))]
    super::audio_out::stop();
    #[cfg(not(any(target_os = "android", feature = "native-audio")))]
    {
        *ffplay_wall().lock().expect("wall lock") = None;
    }
    let song = body["song"].as_str().unwrap_or_default();
    if body["stop"].as_bool() == Some(true) || song.is_empty() {
        return Ok(json!({ "ok": true, "playing": false }));
    }
    let path = song_audio(song).ok_or_else(|| CallError::Bad(format!("no audio for {song}")))?;
    let from = body["from"].as_f64().unwrap_or(0.0).max(0.0);
    if cfg!(test) {
        return Ok(json!({ "ok": true, "playing": true, "from": from }));
    }
    // Android (no ffplay) and `--features native-audio`: cpal, in-process.
    #[cfg(any(target_os = "android", feature = "native-audio"))]
    {
        let device = super::audio_out::start(&path, from).map_err(CallError::Bad)?;
        eprintln!("  audio: {song} from {from:.3}s on {device}");
        return Ok(json!({ "ok": true, "playing": true, "from": from, "native": true }));
    }
    #[cfg(not(any(target_os = "android", feature = "native-audio")))]
    {
        let child = Command::new("ffplay")
            .args(["-nodisp", "-autoexit", "-loglevel", "quiet", "-ss", &format!("{from:.3}")])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| CallError::Bad(format!("ffplay: {e}")))?;
        *slot = Some(child);
        *ffplay_wall().lock().expect("wall lock") = Some((Instant::now(), from));
        Ok(json!({ "ok": true, "playing": true, "from": from }))
    }
}


/// `GET /author/play`: where the sound is, from the audio stream's own cursor
/// (`audio_out`). `at` is seconds into the song. On the desktop the sound is
/// `ffplay`'s, which has no clock the process can read, so `at` is the wall
/// clock from its spawn (`clock: "wall"`) while it runs; `native` is true for
/// either clock.
pub fn play_clock(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    // The lanes playhead as the shell drew it last (`playhead_s`), beside the
    // audio clock (`at`), for measuring the two against each other.
    let (playhead_s, playhead_on) = crate::playhead::published(metal_schema::TickRate::CANONICAL.hz() as f32);
    #[cfg(any(target_os = "android", feature = "native-audio"))]
    {
        let at = super::audio_out::position();
        return Ok(json!({ "playing": super::audio_out::playing(), "at": at, "native": at.is_some(), "playhead_s": playhead_s, "playhead_playing": playhead_on }));
    }
    #[cfg(not(any(target_os = "android", feature = "native-audio")))]
    {
        if let Some(at) = ffplay_clock() {
            return Ok(json!({ "playing": true, "at": at, "native": true, "clock": "wall", "playhead_s": playhead_s, "playhead_playing": playhead_on }));
        }
        Ok(json!({ "playing": false, "at": null, "native": false, "playhead_s": playhead_s, "playhead_playing": playhead_on }))
    }
}

/// Where the editor's HAP proxies live.
pub const HAP: &str = "data/projects/hap";

/// One video's picture: its HAP proxy, or (failing that) MJPEG frames filled
/// by the decoder thread.
#[derive(Default)]
struct Decoded {
    hap: Option<metal_reel::hap::Hap>,
    /// The last HAP frame served, as JPEG: a paused clip is asked the same
    /// frame every repaint.
    served: Option<(usize, Vec<u8>)>,
    frames: Vec<Vec<u8>>,
    done: bool,
    /// Seconds into the clip the editor last asked for.
    at: f64,
}

fn videos() -> &'static Mutex<HashMap<String, Arc<Mutex<Decoded>>>> {
    static V: OnceLock<Mutex<HashMap<String, Arc<Mutex<Decoded>>>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Splits an MJPEG byte stream into whole JPEGs (SOI … EOI); returns the
/// complete ones and leaves the tail in `buf`.
pub fn split_jpegs(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let Some(start) = buf.windows(2).position(|w| w == [0xFF, 0xD8]) else {
            buf.clear();
            break;
        };
        let Some(end) = buf[start + 2..].windows(2).position(|w| w == [0xFF, 0xD9]).map(|e| start + 2 + e + 2) else {
            buf.drain(..start);
            break;
        };
        out.push(buf[start..end].to_vec());
        buf.drain(..end);
    }
    out
}

/// The proxy's file stem: the clip's own stem plus a hash of its full path, so
/// two `intro.mp4` in different folders never share one
/// ([`metal_reel::scenes::slug`], the first half of every scene id).
fn hap_slug(path: &str) -> String {
    metal_reel::scenes::slug(path)
}

/// `path`'s HAP proxy under `root`, made on first use: one
/// [`metal_ingest::video::encode_hap_video`] pass straight from the source, at
/// [`VIDEO_FPS`] and [`VIDEO_W`], with the source's alpha preserved. `on` is
/// handed ffmpeg's own frame progress (`0..1`) as the pass runs.
///
/// 2026-10-01: the same pass cuts the clip into scenes. The descriptor rows
/// `metal_reel::index::cut` reads come from the finished proxy itself (not a
/// ladder of intermediate JPEGs), and each scene's poster is its middle frame
/// read back from that proxy. A proxy made before scenes existed gets its cut
/// list from the proxy itself, on its next open.
pub(super) fn hap_proxy(root: &Path, path: &str, on: &mut dyn FnMut(f32)) -> Result<metal_reel::hap::Hap, String> {
    let slug = hap_slug(path);
    let movie = root.join(format!("{slug}.mov"));
    let index = root.join(format!("{slug}.json"));
    if let Ok(mut hap) = metal_reel::hap::Hap::open(&movie, &index) {
        if metal_reel::scenes::load_cuts(root, &slug).is_none() {
            let rows = metal_reel::scenes::hap_rows(&mut hap);
            if let Err(e) = write_scenes(root, path, &rows, &mut hap) {
                eprintln!("[author] scenes of {path}: {e}");
            }
        }
        return Ok(hap);
    }
    // One probe, three answers: the swscale prefix a hostile DXV source needs
    // before any filter, whether it carries alpha (BC3 over BC1), and the frame
    // count that scales ffmpeg's progress. A source that cannot be probed keeps
    // the old, prefixless chain and no alpha rather than failing here.
    let info = metal_ingest::video::probe(Path::new(path)).ok();
    let prefix = info
        .as_ref()
        .map(|i| metal_ingest::video::normalize_prefix(&i.pix_fmt, &i.color_space))
        .unwrap_or("");
    let alpha = info.as_ref().is_some_and(|i| metal_ingest::video::has_alpha(&i.pix_fmt));
    let total = info
        .map(|i| (i.duration * f64::from(VIDEO_FPS)).round() as u32)
        .unwrap_or(0);
    let (width, height, packets) = metal_ingest::video::encode_hap_video(
        Path::new(path),
        f64::from(VIDEO_FPS),
        VIDEO_W,
        total,
        &movie,
        prefix,
        alpha,
        on,
    )
    .map_err(|e| e.to_string())?;
    let text = serde_json::to_string(&metal_reel::hap::Index { width, height, packets }).map_err(|e| e.to_string())?;
    metal_vfs::write(&index, text).map_err(|e| e.to_string())?;
    let mut hap = metal_reel::hap::Hap::open(&movie, &index)?;
    let rows = metal_reel::scenes::hap_rows(&mut hap);
    if let Err(e) = write_scenes(root, path, &rows, &mut hap) {
        eprintln!("[author] scenes of {path}: {e}");
    }
    Ok(hap)
}

/// The clip's cut list beside its proxy, with a poster per scene; replaces
/// whatever list and posters an earlier ingest left.
fn write_scenes(root: &Path, path: &str, rows: &[[f32; metal_reel::read::COLUMNS]], hap: &mut metal_reel::hap::Hap) -> Result<(), String> {
    let slug = hap_slug(path);
    let mut cuts = metal_reel::scenes::cuts(path, rows, VIDEO_FPS);
    let _ = metal_vfs::remove_dir_all(root.join(format!("{slug}.scenes")));
    for (k, cut) in cuts.iter_mut().enumerate() {
        let poster = metal_reel::scenes::poster(root, &slug, k);
        let Ok(jpeg) = hap_jpeg(hap, cut.rung) else { continue };
        if let Some(dir) = poster.parent() {
            metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        metal_vfs::write(&poster, jpeg).map_err(|e| e.to_string())?;
        cut.poster = Some(poster.to_string_lossy().into_owned());
    }
    metal_reel::scenes::save_cuts(root, &slug, &cuts)
}

/// Starts `path`'s proxy (and so its scenes) in the background, if nothing
/// has yet.
pub(super) fn ingest(path: &str) {
    if !cfg!(test) && metal_vfs::is_file(&Path::new(path)) {
        let _ = decoded(path);
    }
}

/// One HAP frame as a JPEG for the preview.
fn hap_jpeg(hap: &mut metal_reel::hap::Hap, k: usize) -> Result<Vec<u8>, String> {
    let frame = hap.frame(k)?;
    let rgba = frame.rgba();
    let mut out = Vec::new();
    let w = u16::try_from(frame.width).map_err(|e| e.to_string())?;
    let h = u16::try_from(frame.height).map_err(|e| e.to_string())?;
    jpeg_encoder::Encoder::new(&mut out, 85)
        .encode(&rgba, w, h, jpeg_encoder::ColorType::Rgba)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// One RGBA picture as a JPEG for the preview (the native decoder's output;
/// [`hap_jpeg`] is the same for a HAP frame).
#[cfg(target_os = "android")]
fn rgba_jpeg(rgba: &metal_reel::clip::Rgba) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let w = u16::try_from(rgba.width).map_err(|e| e.to_string())?;
    let h = u16::try_from(rgba.height).map_err(|e| e.to_string())?;
    jpeg_encoder::Encoder::new(&mut out, 85)
        .encode(&rgba.pixels, w, h, jpeg_encoder::ColorType::Rgba)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Android has no ffmpeg: the clip is decoded in-process with AMediaExtractor +
/// AMediaCodec, once, front to back. Frames are sampled at [`VIDEO_FPS`] into
/// the same `frames` list the MJPEG pipe fills. The same pass cuts the clip
/// into scenes: each sampled frame's descriptor row is computed as it goes, so
/// no RGB frame list is kept. The length comes from the container when it has
/// one; otherwise the decode runs to end-of-stream and counts what it made.
#[cfg(target_os = "android")]
fn decode_native(path: &str, sink: &Mutex<Decoded>) -> Result<(), String> {
    // One decoder at a time: two clips decoded together stall each other on
    // the device's codec (seen on the emulator), and a phone has one anyway.
    static ONE_AT_A_TIME: OnceLock<Mutex<()>> = OnceLock::new();
    let _turn = ONE_AT_A_TIME.get_or_init(|| Mutex::new(())).lock().expect("decode turn lock");
    match decode_with(path, sink, false) {
        Err(e) if e.contains(metal_reel::clip::media::STALLED) => {
            eprintln!("[author] {path}: {e}; decoding on the software decoder");
            sink.lock().expect("decode lock").frames.clear();
            decode_with(path, sink, true)
        }
        other => other,
    }
}

/// One pass of [`decode_native`] on the default decoder, or the software one.
#[cfg(target_os = "android")]
fn decode_with(path: &str, sink: &Mutex<Decoded>, software: bool) -> Result<(), String> {
    use metal_reel::clip::ClipDecoder;
    let mut clip = metal_reel::clip::media::MediaClip::open_with(Path::new(path), VIDEO_W, software)?;
    let fps = f64::from(VIDEO_FPS);
    let duration = clip.duration();
    let slots = (duration > 0.0).then(|| (duration * fps).ceil() as usize);
    eprintln!("[author] native decode of {path}: opened, {duration:.2} s, software {software}");
    let mut rows: Vec<[f32; metal_reel::read::COLUMNS]> = Vec::new();
    let mut prev: Option<metal_reel::read::Rgb> = None;
    let kept = clip.sample_all(fps, slots, |_, frame| {
        let jpeg = rgba_jpeg(frame)?;
        sink.lock().expect("decode lock").frames.push(jpeg);
        let rgb = rgb_of(frame);
        rows.push(metal_reel::read::describe(&rgb, prev.as_ref()));
        prev = Some(rgb);
        Ok(())
    })?;
    eprintln!("[author] native decode of {path}: {kept} frames");
    if kept == 0 {
        return Err("no frames decoded".into());
    }
    if let Err(e) = write_native_scenes(Path::new(HAP), path, &rows, sink) {
        eprintln!("[author] scenes of {path}: {e}");
    }
    Ok(())
}

/// A decoded picture as RGB, the input of `metal_reel::read::describe`.
#[cfg(target_os = "android")]
fn rgb_of(frame: &metal_reel::clip::Rgba) -> metal_reel::read::Rgb {
    metal_reel::read::Rgb {
        width: frame.width,
        height: frame.height,
        px: frame.pixels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
    }
}

/// The cut list of a natively decoded clip, beside [`HAP`], with a poster per
/// scene taken from the kept JPEG frame at its middle.
#[cfg(target_os = "android")]
fn write_native_scenes(root: &Path, path: &str, rows: &[[f32; metal_reel::read::COLUMNS]], sink: &Mutex<Decoded>) -> Result<(), String> {
    let slug = hap_slug(path);
    let mut cuts = metal_reel::scenes::cuts(path, rows, VIDEO_FPS);
    let d = sink.lock().expect("decode lock");
    let _ = std::fs::remove_dir_all(root.join(format!("{slug}.scenes")));
    for (k, cut) in cuts.iter_mut().enumerate() {
        let Some(jpeg) = d.frames.get(cut.rung) else { continue };
        let poster = metal_reel::scenes::poster(root, &slug, k);
        if let Some(dir) = poster.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&poster, jpeg).map_err(|e| e.to_string())?;
        cut.poster = Some(poster.to_string_lossy().into_owned());
    }
    drop(d);
    metal_reel::scenes::save_cuts(root, &slug, &cuts)
}

/// The picture of `path`, started on first use.
fn decoded(path: &str) -> Arc<Mutex<Decoded>> {
    let mut all = videos().lock().expect("videos lock");
    if let Some(d) = all.get(path) {
        return Arc::clone(d);
    }
    let d = Arc::new(Mutex::new(Decoded::default()));
    all.insert(path.to_string(), Arc::clone(&d));
    let sink = Arc::clone(&d);
    let path = path.to_string();
    std::thread::Builder::new().name("metal-dash decode".into()).spawn(move || {
        match hap_proxy(Path::new(HAP), &path, &mut |_| {}) {
            Ok(hap) => {
                let mut d = sink.lock().expect("decode lock");
                d.hap = Some(hap);
                d.done = true;
                return;
            }
            Err(e) => eprintln!("[author] HAP proxy for {path}: {e}; decoding MJPEG instead"),
        }
        #[cfg(target_os = "android")]
        {
            match decode_native(&path, &sink) {
                Ok(()) => {
                    sink.lock().expect("decode lock").done = true;
                    return;
                }
                Err(e) => eprintln!("[author] native decode of {path}: {e}"),
            }
        }
        let prefix = metal_ingest::video::probe(Path::new(&path))
            .map(|i| metal_ingest::video::normalize_prefix(&i.pix_fmt, &i.color_space))
            .unwrap_or("");
        let child = Command::new("ffmpeg")
            .args(["-loglevel", "quiet", "-i", &path, "-an", "-vf"])
            .arg(format!("{prefix}fps={VIDEO_FPS},scale={VIDEO_W}:-2"))
            .args(["-c:v", "mjpeg", "-q:v", "5", "-f", "image2pipe", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut out) = child.stdout.take() {
                let mut buf = Vec::new();
                let mut chunk = vec![0u8; 1 << 16];
                while let Ok(n) = out.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let frames = split_jpegs(&mut buf);
                    if !frames.is_empty() {
                        sink.lock().expect("decode lock").frames.extend(frames);
                    }
                }
            }
            let _ = child.wait();
        }
        sink.lock().expect("decode lock").done = true;
    })
    .expect("the decode thread starts");
    d
}

pub fn video_seek(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let path = body["path"].as_str().filter(|p| !p.is_empty()).ok_or_else(|| CallError::Bad("path?".into()))?;
    if !metal_vfs::is_file(&Path::new(path)) {
        return Err(CallError::Bad(format!("no such video: {path}")));
    }
    let t = body["t"].as_f64().unwrap_or(0.0).max(0.0);
    if cfg!(test) {
        return Ok(json!({ "ok": true, "t": t }));
    }
    let d = decoded(path);
    let mut d = d.lock().expect("decode lock");
    d.at = t;
    let decoded = d.hap.as_ref().map_or(d.frames.len(), metal_reel::hap::Hap::frames);
    Ok(json!({ "ok": true, "t": t, "decoded": decoded, "done": d.done, "hap": d.hap.is_some() }))
}

pub fn video_frame(_ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let path = params.param("path").filter(|p| !p.is_empty()).ok_or_else(|| CallError::Bad("path?".into()))?;
    let d = videos().lock().expect("videos lock").get(path).cloned().ok_or_else(|| CallError::Bad("seek first".into()))?;
    let mut d = d.lock().expect("decode lock");
    let i = (d.at * f64::from(VIDEO_FPS)).floor() as usize;
    if let Some(count) = d.hap.as_ref().map(metal_reel::hap::Hap::frames) {
        let k = i.min(count.saturating_sub(1));
        if let Some((_, jpeg)) = d.served.as_ref().filter(|(at, _)| *at == k) {
            return Ok(Bytes { kind: "image/jpeg", body: jpeg.clone() });
        }
        let hap = d.hap.as_mut().expect("checked above");
        let jpeg = hap_jpeg(hap, k).map_err(CallError::Bad)?;
        d.served = Some((k, jpeg.clone()));
        return Ok(Bytes { kind: "image/jpeg", body: jpeg });
    }
    let frame = d.frames.get(i.min(d.frames.len().saturating_sub(1))).ok_or_else(|| CallError::Bad("decoding…".into()))?;
    Ok(Bytes { kind: "image/jpeg", body: frame.clone() })
}

/// `data/projects/<song>.json` — the song's name made a safe file name.
fn project_path(song: &str) -> Option<PathBuf> {
    let name = Path::new(song).file_name()?.to_str()?;
    (!name.is_empty() && !name.starts_with('.')).then(|| Path::new(PROJECTS).join(format!("{name}.json")))
}

pub fn project_get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let song = params.param("song").unwrap_or_default();
    let path = project_path(song).ok_or_else(|| CallError::Bad("song?".into()))?;
    match metal_vfs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| CallError::Bad(format!("{}: {e}", path.display()))),
        Err(_) => Ok(json!({ "song": song, "clips": [] })),
    }
}

pub fn project_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let song = body["song"].as_str().unwrap_or_default();
    let path = project_path(song).ok_or_else(|| CallError::Bad("song?".into()))?;
    if !body["clips"].is_array() {
        return Err(CallError::Bad("clips?".into()));
    }
    let _ = metal_vfs::create_dir_all(PROJECTS);
    let text = serde_json::to_string_pretty(&body).map_err(|e| CallError::Bad(e.to_string()))?;
    metal_vfs::write(&path, text).map_err(|e| CallError::Bad(format!("{}: {e}", path.display())))?;
    Ok(json!({ "ok": true, "clips": body["clips"].as_array().map_or(0, Vec::len) }))
}

// `metal-sync`'s control port: defined in `call::peers` (ungated), re-exported here.
pub use crate::call::peers::sync_port;
#[cfg(test)]
pub use crate::call::peers::sync_port_from;

/// One request to `metal-sync` on loopback: its JSON reply, or `None` when
/// nothing answers.
fn sync_json(method: &str, path: &str) -> Option<Value> {
    use std::io::{Read, Write};
    let port = sync_port();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
    write!(s, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").ok()?;
    let mut text = String::new();
    s.read_to_string(&mut text).ok()?;
    serde_json::from_str(text.split_once("\r\n\r\n")?.1).ok()
}

/// A query value: unreserved ASCII as is, every other byte as `%XX`.
fn query_value(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `GET /author/song/pull?song=`: whether this node has the song's audio, and
/// the pull's progress from `metal-sync` (`null` when no pull ran, or sync is down).
pub fn song_pull_get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let song = params.param("song").filter(|s| !s.is_empty()).ok_or_else(|| CallError::Bad("song?".into()))?;
    let pull = sync_json("GET", &format!("/song/pull?song={}", query_value(song)));
    Ok(json!({
        "song": song,
        "has_audio": song_audio(song).is_some(),
        "reachable": pull.is_some(),
        "pull": pull.unwrap_or(Value::Null),
    }))
}

/// `POST /author/song/pull {song}`: asks `metal-sync` to pull the song's bundle
/// from a paired device. Answers at once; progress is read back with the GET.
pub fn song_pull_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let song = body["song"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| CallError::Bad("song?".into()))?;
    let started = sync_json("POST", &format!("/song/pull?song={}", query_value(song))).is_some();
    Ok(json!({ "song": song, "started": started }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the loops design notes T15: the sidecar beside the song is what playback reads;
    /// no sidecar → no beat map (the live/free clock is kept).
    #[test]
    fn the_sync_port_is_configurable_and_defaults_to_7901() {
        assert_eq!(sync_port_from(None), 7901);
        assert_eq!(sync_port_from(Some("")), 7901);
        assert_eq!(sync_port_from(Some("nope")), 7901);
        assert_eq!(sync_port_from(Some("0")), 7901);
        assert_eq!(sync_port_from(Some("7911")), 7911);
        assert_eq!(sync_port_from(Some(" 17901 ")), 17901);
    }

    #[test]
    fn playback_reads_the_songs_tempo_sidecar_or_falls_back() {
        let dir = std::env::temp_dir().join(format!("fw-play-sidecar-{}", std::process::id()));
        metal_vfs::create_dir_all(&dir).unwrap();
        let audio = dir.join("tune.mp3");
        assert_eq!(sidecar(&audio), dir.join("tune.tempo.json"));
        assert!(tempo_track(&audio).is_none(), "missing sidecar falls back");
        let map = metal_listen::file::TempoMap {
            segments: vec![(0.0, 120.0)],
            beats: vec![0.5, 1.0, 1.5, 2.0, 2.5],
            downbeats: vec![1.0],
            tempo_confidence: 0.9,
            downbeat_confidence: 0.8,
        };
        metal_vfs::write(sidecar(&audio), map.to_json()).unwrap();
        let track = tempo_track(&audio).expect("sidecar read back");
        assert_eq!(track.downbeat(), Some(1));
        assert_eq!(track.at(1.25), (Some(1.5), Some(1), Some(120.0)));
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// A stream cut mid-frame keeps its tail for the next read.
    #[test]
    fn split_jpegs_returns_whole_frames_and_keeps_the_tail() {
        let mut buf = vec![0x00, 0xFF, 0xD8, 1, 2, 0xFF, 0xD9, 0xFF, 0xD8, 3];
        let frames = split_jpegs(&mut buf);
        assert_eq!(frames, vec![vec![0xFF, 0xD8, 1, 2, 0xFF, 0xD9]]);
        assert_eq!(buf, vec![0xFF, 0xD8, 3]);
        buf.extend_from_slice(&[0xFF, 0xD9]);
        assert_eq!(split_jpegs(&mut buf).len(), 1);
        assert!(buf.is_empty());
    }

    #[test]
    fn a_hap_slug_keeps_the_stem_and_tells_folders_apart() {
        let a = hap_slug("/a/intro clip.mp4");
        assert!(a.starts_with("intro_clip-"));
        assert_ne!(a, hap_slug("/b/intro clip.mp4"));
    }

    /// A real clip → its proxy → a JPEG frame, and the second open reuses the
    /// file. Needs ffmpeg with the `hap` encoder; skipped without it.
    #[test]
    fn a_video_becomes_a_hap_proxy_the_editor_reads() {
        let ok = Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output();
        if !ok.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(" hap ")) {
            eprintln!("skipped: no ffmpeg hap encoder");
            return;
        }
        let dir = std::env::temp_dir().join(format!("fw-playout-hap-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.mp4");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=24:duration=2"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let root = dir.join("hap");
        let mut hap = hap_proxy(&root, src.to_str().unwrap(), &mut |_| {}).unwrap();
        assert_eq!(hap.frames(), 16, "2 s at 8 fps");
        assert_eq!((hap.index.width, hap.index.height), (480, 360));
        let jpeg = hap_jpeg(&mut hap, 5).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        let again = hap_proxy(&root, src.to_str().unwrap(), &mut |_| {}).unwrap();
        assert_eq!(again.index, hap.index);
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The zscale wire-up, on the only clip this build can make. A DXV/YCGCO
    /// clip cannot be synthesised here (the `dxv` encoder takes only rgba/dxt1),
    /// so this pins that an ordinary clip probes to no prefix — its chain is
    /// unchanged — and that its proxy still builds. The prefix itself is
    /// guarded by `metal-ingest`'s pure unit tests.
    #[test]
    fn a_plain_clip_needs_no_prefix_and_still_builds_its_proxy() {
        let ok = Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output();
        if !ok.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(" hap ")) {
            eprintln!("skipped: no ffmpeg hap encoder");
            return;
        }
        let dir = std::env::temp_dir().join(format!("fw-playout-prefix-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let src = dir.join("plain.mp4");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=24:duration=1"])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let info = metal_ingest::video::probe(&src).expect("probe");
        assert_eq!(info.pix_fmt, "yuv420p");
        assert_eq!(
            metal_ingest::video::normalize_prefix(&info.pix_fmt, &info.color_space),
            "",
            "a normal clip carries no prefix"
        );
        let mut hap = hap_proxy(&dir.join("hap"), src.to_str().unwrap(), &mut |_| {}).expect("proxy builds");
        assert!(hap.frames() > 0);
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// Three shots, two cuts → three scenes from the one ingest pass, each
    /// with a poster taken from the proxy; a favourite outlives deleting the
    /// proxy and ingesting again, because the cutter gives the same ids.
    #[test]
    fn a_two_cut_clip_yields_its_scenes_and_a_favourite_survives_a_re_ingest() {
        let ok = Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output();
        if !ok.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(" hap ")) {
            eprintln!("skipped: no ffmpeg hap encoder");
            return;
        }
        let dir = std::env::temp_dir().join(format!("fw-playout-scenes-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let src = dir.join("cuts.mp4");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc=size=320x240:rate=24:duration=3"])
            .args(["-f", "lavfi", "-i", "color=c=red:size=320x240:rate=24:duration=3"])
            .args(["-f", "lavfi", "-i", "smptebars=size=320x240:rate=24:duration=3"])
            .args(["-filter_complex", "[0:v][1:v][2:v]concat=n=3:v=1:a=0", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let (clip, root, book) = (src.to_str().unwrap(), dir.join("hap"), dir.join("favorites.json"));
        hap_proxy(&root, clip, &mut |_| {}).unwrap();
        let listed = super::super::scenes::clip_scenes(&root, &book, clip);
        let scenes = listed["scenes"].as_array().unwrap();
        assert_eq!(scenes.len(), 3, "{listed}");
        assert!((scenes[1]["start"].as_f64().unwrap() - 3.0).abs() < 0.2);
        for s in scenes {
            assert!(metal_vfs::is_file(&Path::new(s["poster"].as_str().unwrap())));
        }
        let id = scenes[1]["id"].as_str().unwrap().to_string();
        let mut picked = scenes[1].clone();
        picked["op"] = json!("favourite");
        picked["on"] = json!(true);
        super::super::scenes::apply_at(&book, &picked).unwrap();

        metal_vfs::remove_dir_all(&root).unwrap();
        hap_proxy(&root, clip, &mut |_| {}).unwrap();
        let again = super::super::scenes::clip_scenes(&root, &book, clip);
        let row = again["scenes"].as_array().unwrap().iter().find(|s| s["id"] == id.as_str()).cloned().unwrap();
        assert_eq!(row["favourite"], true);
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_file_is_named_after_the_song_alone() {
        assert_eq!(project_path("a b"), Some(Path::new(PROJECTS).join("a b.json")));
        assert_eq!(project_path("../../etc/x"), Some(Path::new(PROJECTS).join("x.json")));
        assert_eq!(project_path(""), None);
        assert_eq!(project_path(".hidden"), None);
    }
}

#[cfg(all(test, not(any(target_os = "android", feature = "native-audio"))))]
mod ffplay_clock_tests {
    use super::*;

    /// A running child stands in for ffplay: the clock reads the song second
    /// it started from plus the wall time since, and stops with the child.
    #[test]
    fn the_wall_clock_runs_while_the_child_does_and_stops_with_it() {
        let mut child = Command::new("sleep").arg("30").spawn().expect("sleep");
        *player().lock().unwrap() = Some(child);
        *ffplay_wall().lock().unwrap() = Some((Instant::now() - std::time::Duration::from_secs(2), 1.0));
        let at = ffplay_clock().expect("running child has a clock");
        assert!((3.0..3.5).contains(&at), "1 s from plus 2 s of wall: {at}");

        if let Some(mut c) = player().lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        assert_eq!(ffplay_clock(), None, "a stopped child has no clock");
        *ffplay_wall().lock().unwrap() = None;
    }
}
