//! 2026-10-10: song audio out of the process, through cpal's default output.
//!
//! `playout::play` used to spawn `ffplay`, which does not exist on the phone.
//! Here the song is decoded whole (`metal_listen::file::decode_pcm`, symphonia in
//! pure Rust) and streamed by a cpal output stream: AAudio on Android (cpal's
//! oboe backend), ALSA/Pulse on desktop. The stream's callback owns the
//! cursor; `position()` reads it, so the editor can ask where the sound is.
//!
//! Built on Android always, and on desktop with `--features native-audio`;
//! the desktop default keeps `ffplay`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use metal_listen::file::Pcm;

/// What the callback and the control thread share.
struct Shared {
    /// The source frame the next output frame reads, in whole frames.
    cursor: AtomicU64,
    /// Set by the callback once the cursor passes the last frame.
    finished: AtomicBool,
    rate: u32,
    /// The device clock at the last callback. `None` until the first one runs.
    anchor: Mutex<Option<Anchor>>,
}

/// What one callback reported: the source frame the device was given first
/// (the stream position), the output latency cpal predicted for it, and the
/// wall-clock instant the callback ran.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub frame: u64,
    pub latency: Duration,
    pub at: Instant,
}

/// Seconds into the song that the speakers are at `since` after `anchor.at`.
///
/// The frame the device was handed at the callback is heard `latency` later,
/// so what is audible at the callback is `frame / rate - latency`, and it moves
/// on at the wall clock from there. Clamped at the start of the song.
pub fn audible_at(frame: u64, rate: u32, latency: Duration, since: Duration) -> f64 {
    let heard = frame as f64 / f64::from(rate.max(1)) - latency.as_secs_f64();
    (heard + since.as_secs_f64()).max(0.0)
}

/// One song playing. The cpal stream is not `Send` on every backend (oboe's
/// is not), so it never leaves the thread that opened it: that thread holds
/// the stream until `stop` is dropped, and this handle keeps only the sender.
pub struct Playing {
    stop: mpsc::Sender<()>,
    shared: Arc<Shared>,
}

fn slot() -> &'static Mutex<Option<Playing>> {
    static P: OnceLock<Mutex<Option<Playing>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// Stops whatever is playing here (its thread drops the stream).
pub fn stop() {
    if let Some(p) = slot().lock().expect("audio slot lock").take() {
        let _ = p.stop.send(());
    }
}

/// Seconds into the song the sound is at (the stream position less the
/// device's output latency, moved on by the wall clock since the last
/// callback), or `None` when nothing plays here. Before the first callback it
/// is the start point.
pub fn position() -> Option<f64> {
    let guard = slot().lock().expect("audio slot lock");
    let p = guard.as_ref()?;
    let anchor = *p.shared.anchor.lock().expect("audio anchor lock");
    Some(match anchor {
        Some(a) => audible_at(a.frame, p.shared.rate, a.latency, a.at.elapsed()),
        None => p.shared.cursor.load(Ordering::Relaxed) as f64 / f64::from(p.shared.rate),
    })
}

/// True while the song is still sounding (false after its last frame).
pub fn playing() -> bool {
    slot()
        .lock()
        .expect("audio slot lock")
        .as_ref()
        .is_some_and(|p| !p.shared.finished.load(Ordering::Relaxed))
}

/// Decodes `path` and starts it at `from` seconds on the default output,
/// replacing whatever played before. Returns the device name it opened.
pub fn start(path: &Path, from: f64) -> Result<String, String> {
    stop();
    let pcm = Arc::new(metal_listen::file::decode_pcm(path)?);
    let shared = Arc::new(Shared {
        cursor: AtomicU64::new((from.max(0.0) * f64::from(pcm.rate)) as u64),
        finished: AtomicBool::new(false),
        rate: pcm.rate,
        anchor: Mutex::new(None),
    });
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<String, String>>();
    let shared_t = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("fw-audio".into())
        .spawn(move || {
            let opened = open(&pcm, &shared_t);
            match opened {
                Ok((stream, name)) => {
                    let _ = ready_tx.send(Ok(name));
                    // Hold the stream until told to stop, or until the handle
                    // is dropped by a newer `start`.
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| format!("audio thread: {e}"))?;
    let device = ready_rx.recv().map_err(|_| "audio thread ended".to_string())??;
    *slot().lock().expect("audio slot lock") = Some(Playing { stop: stop_tx, shared });
    Ok(device)
}

/// Opens and starts the default output stream for `pcm`.
fn open(pcm: &Arc<Pcm>, shared: &Arc<Shared>) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output device")?;
    let name = device.name().unwrap_or_else(|_| "default".into());
    let supported = device.default_output_config().map_err(|e| format!("output config: {e}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, pcm, shared)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, pcm, shared)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, pcm, shared)?,
        other => return Err(format!("unsupported output sample format {other:?}")),
    };
    stream.play().map_err(|e| format!("starting the output: {e}"))?;
    Ok((stream, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    pcm: &Arc<Pcm>,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let out_channels = usize::from(config.channels);
    let src_channels = pcm.channels.max(1);
    // Source frames per output frame: the resampler is linear interpolation
    // between the two source frames around the cursor.
    let step = f64::from(pcm.rate) / f64::from(config.sample_rate.0);
    let pcm = Arc::clone(pcm);
    let shared_cb = Arc::clone(shared);
    let mut pos = shared.cursor.load(Ordering::Relaxed) as f64;
    let out_rate = f64::from(config.sample_rate.0);
    device
        .build_output_stream(
            config,
            move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
                let frames = pcm.samples.len() / src_channels;
                let stamp = info.timestamp();
                let reported = stamp.playback.duration_since(&stamp.callback).unwrap_or_default();
                // Some backends report a playback stamp on another clock base:
                // the emulator's AAudio reported ~376 s of latency, which pinned
                // the clock at the song start. Anything over a second is not a
                // buffer latency; use this callback's buffer length instead.
                let buffered = (out.len() / out_channels) as f64 / out_rate;
                let latency = if reported > Duration::from_secs(1) {
                    Duration::from_secs_f64(buffered)
                } else {
                    reported
                };
                // The lock is `try_lock` so the callback never waits on a reader.
                if let Ok(mut anchor) = shared_cb.anchor.try_lock() {
                    *anchor = Some(Anchor {
                        frame: pos as u64,
                        latency,
                        at: Instant::now(),
                    });
                }
                for frame in out.chunks_mut(out_channels) {
                    let i = pos as usize;
                    if i + 1 >= frames {
                        shared_cb.finished.store(true, Ordering::Relaxed);
                        frame.iter_mut().for_each(|s| *s = T::from_sample(0.0_f32));
                        continue;
                    }
                    let t = (pos - i as f64) as f32;
                    for (c, s) in frame.iter_mut().enumerate() {
                        // Source channel for this output channel: mono
                        // feeds every output, extra outputs stay silent.
                        let v = if src_channels == 1 {
                            lerp(&pcm.samples, i, 1, 0, t)
                        } else if c < src_channels {
                            lerp(&pcm.samples, i, src_channels, c, t)
                        } else {
                            0.0
                        };
                        *s = T::from_sample(v);
                    }
                    pos += step;
                }
                shared_cb.cursor.store(pos as u64, Ordering::Relaxed);
            },
            |e| eprintln!("  audio: output error: {e}"),
            None,
        )
        .map_err(|e| format!("opening the output: {e}"))
}

/// Interpolated sample `c` between source frames `i` and `i + 1`.
fn lerp(samples: &[f32], i: usize, channels: usize, c: usize, t: f32) -> f32 {
    let a = samples[i * channels + c];
    let b = samples[(i + 1) * channels + c];
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    /// At the callback, the frame handed over is heard `latency` later, so the
    /// audible position is the stream position less the latency.
    #[test]
    fn audible_is_the_stream_position_less_the_output_latency() {
        // 48 kHz, stream at 10 s, 40 ms of output latency.
        let at = audible_at(480_000, 48_000, MS(40), Duration::ZERO);
        assert!((at - 9.96).abs() < 1e-9, "got {at}");
    }

    /// Between callbacks it moves on at the wall clock, from that anchor.
    #[test]
    fn audible_moves_with_the_wall_clock_since_the_callback() {
        let at = audible_at(480_000, 48_000, MS(40), MS(250));
        assert!((at - 10.21).abs() < 1e-9, "got {at}");
    }

    /// A latency longer than the stream position clamps at the song's start,
    /// and so does a negative reading, never a negative time.
    #[test]
    fn audible_never_goes_before_the_start() {
        assert_eq!(audible_at(0, 48_000, MS(120), Duration::ZERO), 0.0);
        assert_eq!(audible_at(24_000, 48_000, MS(500), Duration::ZERO), 0.0);
    }

    /// The source rate sets the scale, not the device's: 44.1 kHz at frame
    /// 44_100 is one second whatever the output runs at.
    #[test]
    fn audible_uses_the_source_rate() {
        let at = audible_at(44_100, 44_100, Duration::ZERO, Duration::ZERO);
        assert!((at - 1.0).abs() < 1e-9);
    }
}
