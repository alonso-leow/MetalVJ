//! `export.video`'s Offline executor.
//!
//! The catalog kind `export.video` (`metal_graph::kind_domains`) only exists
//! in the [`metal_graph::Domain::Offline`] domain — it is a file, not a
//! device, so a Live graph cannot name it (lint says so). Until now that was
//! the whole of it: a kind and a lint, with no executor behind it. This module
//! is the executor the plan asks for (§9.1 E1): it walks the offline graph's
//! frames and hands each one to the exporter in `metal-render::headless`.
//!
//! # Why this is a `FrameSource` and not a renderer
//!
//! The frame order is the offline graph's own, at the rate the export asked
//! for: frame `k` of the range is `(start + k) / fps` seconds into the graph's
//! timeline — the same "tick is what must render, the same way every run"
//! promise `metal-dash`'s `author::models::timeline` resolves a tick to a
//! graph. The *drawing* is `Renderer::solo_chain`'s, one layer up. So this
//! crate owns the walking (graph → `FrameJob`) and the neutral
//! [`FrameSource`] currency lives in `metal-ingest`, which is what lets
//! `metal-render`'s exporter consume it without either crate depending on the
//! other's timeline or device types.
//!
//! # What one frame is
//!
//! E1 renders **one generator node solo** per frame — the node the export
//! names, or the graph's first authored generator. Audio is not analysed
//! here (E1 is the video engine; the audio mux is E3), so the row is the
//! operator's base row at rest (`Drive::at_rest`), with the graph's own
//! `fx.*` chain baked in exactly as `Paint::run` would
//! ([`crate::chain_for_row`]). That row does not change from frame to frame,
//! so it is computed once; only `time` advances.
use crate::drive::{self, Drive};
use crate::listeners::Digests;
use crate::paint::Paint;
use metal_graph::Graph;
use metal_ingest::audio::{self as ingest_audio, AudioTrack};
use metal_ingest::export::{FrameJob, FrameSource};
use metal_schema::EffectStep;
use std::path::Path;

/// One offline range of a graph, rendered as the exporter's [`FrameSource`].
pub struct Offline {
    index: usize,
    row: Vec<f32>,
    chain: Vec<EffectStep>,
    start: u64,
    total: u64,
    fps: f64,
}

impl Offline {
    /// Walks `[start, end)` of `graph`'s frames at `fps`, rendering generator
    /// node `node`.
    ///
    /// `paint` is read only for its ids and schemas — the model `gen.<id>`
    /// names, resolved to the same index the renderer's `solo_chain` takes,
    /// and the schema the base row comes from. Pass the painter the export
    /// will draw with; only those two lookups are used.
    pub fn new(
        graph: &Graph,
        node: &str,
        start: u64,
        end: u64,
        fps: f64,
        paint: &dyn Paint,
    ) -> Result<Offline, String> {
        if end < start {
            return Err(format!("export range {start}..{end} is reversed"));
        }
        if !(fps > 0.0) {
            return Err(format!("fps {fps} is not a frame rate"));
        }
        let n = graph
            .node(node)
            .ok_or_else(|| format!("no node {node:?} in graph {:?}", graph.id))?;
        let model_id = n
            .kind
            .strip_prefix("gen.")
            .ok_or_else(|| format!("node {node:?} is {:?}, not a generator", n.kind))?;
        let ids = paint.ids();
        let index = ids
            .iter()
            .position(|id| *id == model_id)
            .ok_or_else(|| format!("this build has no model {model_id:?}"))?;
        let schemas = paint.schemas();
        let schema = schemas[index];
        let base = schema.defaults();
        let dials = Drive::of_node(graph.node(node));
        let mut row = drive::row(schema, &base, &Digests::zero(), &dials.at_rest());
        // The graph's `fx.*` chain, in graph order, baked into `row` exactly
        // as `Paint::run` bakes it. Empty when the node carries none, which
        // `solo_chain` reads as the model's legacy order.
        let chain = crate::chain_for_row(graph, node, &mut row, &[]);
        Ok(Offline {
            index,
            row,
            chain,
            start,
            total: end - start,
            fps,
        })
    }

    /// The frame `k` (0-based within the range) will draw at, in seconds.
    pub fn time_of(&self, k: u64) -> f32 {
        (self.start + k) as f32 / self.fps as f32
    }
}

impl FrameSource for Offline {
    fn total(&self) -> u64 {
        self.total
    }

    fn job(&mut self, k: u64) -> Option<FrameJob> {
        (k < self.total).then(|| FrameJob {
            index: self.index,
            floats: self.row.clone(),
            time: self.time_of(k),
            fondo_alpha: false,
            chain: self.chain.clone(),
        })
    }
}

// -- the audio.file executor (plan §9.1 E3) -----------------------------------

/// Rate and channel count the export muxes at: 48 kHz stereo, what AAC and a
/// back-of-the-room PA both want without a second conversion.
pub const EXPORT_RATE: u32 = 48_000;
/// Stereo — see [`EXPORT_RATE`].
pub const EXPORT_CHANNELS: u16 = 2;

/// The `audio.file` Offline executor: the decoded audio of one song/audio file
/// (or the sum of several), yielded as the mix for the export range.
///
/// This is the audio twin of [`Offline`]. Where that walks the graph's picture
/// frames, this owns the file the graph's `audio.file` node names: it decodes
/// it (a WAV here; any compressed format through `metal-listen` when this build
/// has the `listen` feature), mixes several sources together, and slices the
/// result to the seconds an export range covers. The [`AudioTrack`] it yields
/// is what `metal-ingest::audio::write_wav` turns into the encoder's second
/// input.
pub struct AudioFile {
    track: AudioTrack,
}

impl AudioFile {
    /// Decodes one file.
    pub fn load(path: &Path) -> Result<AudioFile, String> {
        Ok(AudioFile { track: decode_audio(path)? })
    }

    /// Decodes and mixes several files — the graph's `audio.file` nodes — at
    /// the export's target rate/channels. Empty input is an error, not a
    /// silent track: a caller that asked for `"mix"` and found no source should
    /// be told, not handed silence.
    pub fn mix(paths: &[std::path::PathBuf]) -> Result<AudioFile, String> {
        if paths.is_empty() {
            return Err("no audio file to mix".into());
        }
        let mut sources = Vec::with_capacity(paths.len());
        for p in paths {
            sources.push(decode_audio(p)?);
        }
        Ok(AudioFile { track: ingest_audio::mix(&sources, EXPORT_RATE, EXPORT_CHANNELS) })
    }

    /// The decoded track at its own rate/channels.
    pub fn track(&self) -> &AudioTrack {
        &self.track
    }

    /// The samples covering frames `[start, end)` at `fps`, converted to
    /// `rate`/`channels`. A non-positive `fps` yields silence rather than a
    /// division by zero (the caller validates `fps` upstream; this is the floor).
    pub fn range(&self, start: u64, end: u64, fps: f64, rate: u32, channels: u16) -> AudioTrack {
        if !(fps > 0.0) || end <= start {
            return AudioTrack::silence(rate, channels, 0);
        }
        let from = start as f64 / fps;
        let to = end as f64 / fps;
        self.track.slice_seconds(from, to).to_rate_channels(rate, channels)
    }
}

/// Decodes one audio file to a track: a WAV through [`ingest_audio::read_wav`]
/// (no decoder needed), everything else through `metal-listen`'s symphonia
/// path when the `listen` feature is on.
pub fn decode_audio(path: &Path) -> Result<AudioTrack, String> {
    let is_wav = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    if is_wav {
        return ingest_audio::read_wav(path);
    }
    #[cfg(feature = "listen")]
    {
        let pcm = metal_listen::file::decode_pcm(path)?;
        Ok(AudioTrack { rate: pcm.rate, channels: pcm.channels.max(1) as u16, samples: pcm.samples })
    }
    #[cfg(not(feature = "listen"))]
    {
        Err(format!("{}: this build decodes WAV only (build with the `listen` feature)", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stand;
    use metal_graph::{Domain, Endpoint};
    use metal_ingest::export::{self as ingest_export, EncodeSettings};

    /// A minimal offline graph: one generator feeding `export.video`, built
    /// the way `author::models::timeline::resolve` builds one, but without the
    /// timeline layer (that lives in `metal-dash`; this test only needs a
    /// graph).
    fn offline_graph() -> (Graph, String, String) {
        let mut g = Graph::new("exp", "Export", Domain::Offline);
        let _audio = g.add_node("audio", "audio.file", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [2.0, 0.0]);
        let out = g.add_node("export", "export.video", [4.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        (g, gen, out)
    }

    #[test]
    fn the_executor_walks_the_range_in_order_at_the_asked_rate() {
        let painter = stand::Stand::new();
        let (g, gen, _out) = offline_graph();
        let mut src = Offline::new(&g, &gen, 10, 15, 8.0, &painter).unwrap();
        assert_eq!(src.total(), 5);
        let ids = painter.ids();
        let want_index = ids.iter().position(|id| *id == "turing").unwrap();
        for k in 0..5 {
            let job = src.job(k).expect("in range");
            assert_eq!(job.index, want_index, "the generator's own model");
            assert!((job.time - (10 + k) as f32 / 8.0).abs() < 1e-6, "frame {k} time");
            assert!(job.floats.len() >= metal_schema::EFFECTS_WIDTH, "a full row");
        }
        assert!(src.job(5).is_none(), "past the range is None");
    }

    #[test]
    fn a_node_that_is_not_a_generator_is_refused() {
        let painter = stand::Stand::new();
        let (g, _gen, out) = offline_graph();
        let err = Offline::new(&g, &out, 0, 1, 8.0, &painter)
            .err()
            .expect("export.video is not a generator");
        assert!(err.contains("not a generator"), "{err}");
        let err = Offline::new(&g, "nope", 0, 1, 8.0, &painter)
            .err()
            .expect("no such node");
        assert!(err.contains("no node"), "{err}");
    }

    /// The whole perform-side path, CPU only: the executor's frames are drawn
    /// through the real `Paint` boundary (`Stand`, the no-GPU painter) and fed
    /// to a real `Encoder`, landing a real file. That is the offline executor
    /// driving an export end to end without a GPU — the GPU renderer is
    /// `metal-render`'s exporter, tested there.
    #[test]
    fn an_offline_range_renders_and_encodes_to_a_real_file() {
        let (w, h) = (320u32, 180u32);
        let dir = std::env::temp_dir().join(format!("fw-perform-export-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.mp4");

        let mut painter = stand::Stand::new();
        let (g, gen, _out) = offline_graph();
        let mut src = Offline::new(&g, &gen, 0, 5, 8.0, &painter).unwrap();

        let mut encoder = match ingest_export::open(&EncodeSettings::new(&path, w, h, 8.0)) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("skipping: ffmpeg unavailable");
                let _ = metal_vfs::remove_dir_all(&dir);
                return;
            }
        };
        let mut done = 0u64;
        for k in 0..src.total() {
            let job = src.job(k).expect("in range");
            let (pixels, pw, ph) =
                painter.solo_chain(job.index, &job.floats, job.time, w, h, job.fondo_alpha, &job.chain);
            assert_eq!((pw, ph), (w, h));
            encoder.push(&pixels).unwrap();
            done += 1;
        }
        let bytes = encoder.finish().unwrap();
        assert_eq!(done, 5, "the executor reported every frame");
        assert!(bytes > 0, "a real file was written");
        assert!(std::path::Path::new(&path).exists());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    fn tone(rate: u32, channels: u16, frames: usize, hz: f64, amp: f32) -> AudioTrack {
        let mut samples = Vec::with_capacity(frames * channels as usize);
        for n in 0..frames {
            let v = amp * (2.0 * std::f64::consts::PI * hz * n as f64 / rate as f64).sin() as f32;
            for _ in 0..channels {
                samples.push(v);
            }
        }
        AudioTrack { rate, channels, samples }
    }

    /// The `audio.file` executor end to end, no ffmpeg: two songs sum over the
    /// exact frame range the export asked for, at the requested rate/channels.
    /// This is the mix the export muxes — E3's "sums two tones to the expected
    /// samples" at the executor's own boundary.
    #[test]
    fn an_export_audio_mix_sums_two_songs_over_the_range() {
        let dir = std::env::temp_dir().join(format!("fw-perform-audio-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        let (rate, frames) = (48_000u32, 4800usize);
        let a = tone(rate, 1, frames, 220.0, 0.2);
        let b = tone(rate, 1, frames, 440.0, 0.3);
        let (pa, pb) = (dir.join("a.wav"), dir.join("b.wav"));
        metal_ingest::audio::write_wav(&pa, &a).unwrap();
        metal_ingest::audio::write_wav(&pb, &b).unwrap();

        let mix = AudioFile::mix(&[pa, pb]).expect("two songs decode and mix");
        // Range 0..2400 frames at 48 kHz = the first 0.05 s, mono at the target.
        let r = mix.range(0, 2400, 48_000.0, rate, 1);
        assert_eq!(r.rate, rate);
        assert_eq!(r.channels, 1);
        assert_eq!(r.frames(), 2400);
        for i in 0..2400 {
            let want = a.samples[i] + b.samples[i];
            assert!(
                (r.samples[i] - want).abs() < 3e-4,
                "frame {i}: {} vs {want} (two 16-bit quantisations apart at most)",
                r.samples[i]
            );
        }
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The mix at the export target is what the encoder muxes: a WAV written
    /// from the executor's range, fed to a real encoder, lands a container
    /// ffprobe reports as carrying both a picture and a sound.
    #[test]
    fn an_export_with_audio_muxes_a_second_stream() {
        use metal_schema::platform::Command;
        let dir = std::env::temp_dir().join(format!("fw-perform-mux-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        let song = dir.join("song.wav");
        metal_ingest::audio::write_wav(&song, &tone(EXPORT_RATE, 2, EXPORT_RATE as usize / 4, 440.0, 0.4))
            .unwrap();
        let audio = AudioFile::load(&song).unwrap();
        let track = audio.range(0, 4, 8.0, EXPORT_RATE, EXPORT_CHANNELS);
        let wav = dir.join("range.wav");
        metal_ingest::audio::write_wav(&wav, &track).unwrap();

        let out = dir.join("out.mp4");
        let mut settings = EncodeSettings::new(&out, 320, 180, 8.0);
        settings.audio = Some(wav);
        let mut encoder = match ingest_export::open(&settings) {
            Ok(e) => e,
            Err(_) => {
                eprintln!("skipping: ffmpeg unavailable");
                let _ = metal_vfs::remove_dir_all(&dir);
                return;
            }
        };
        let (w, h) = (320usize, 180usize);
        for k in 0..4u32 {
            let mut frame = vec![0u8; w * h * 4];
            for (i, px) in frame.chunks_exact_mut(4).enumerate() {
                px[0] = ((i + k as usize) & 255) as u8;
                px[3] = 255;
            }
            encoder.push(&frame).unwrap();
        }
        encoder.finish().unwrap();

        let out = Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .expect("ffprobe runs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("video"), "no video stream: {text:?}");
        assert!(text.contains("audio"), "no audio stream: {text:?}");
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
