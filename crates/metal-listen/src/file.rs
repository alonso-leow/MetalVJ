//! Whole files off disk — the Rust replacement for `digest_audio.py`.
//!
//! The Python digester needed librosa only for two things: decoding the file
//! (`librosa.load`) and the sixteen columns. The columns were already here —
//! [`Digester`] in [`Compat::Librosa`] is the same matrix, measured against
//! librosa by `tests/reference.rs`. What was missing was the decoder, and
//! that is [symphonia](https://docs.rs/symphonia): pure Rust, no system
//! library, mp3/wav/flac/ogg/aac.
//!
//! The path is the one `librosa.load(path, sr=22050, mono=True)` takes:
//! decode → fold to mono → resample to [`crate::SR`] → digest → [`Digester::finish`].
//! The output is the script's `(rows, 16)` matrix extended to `(rows, 35)`
//! with the voice then tone blocks, so the music prefix is still the one
//! `digest_audio.py` would have got and only the appended columns are new.
//!
//! # The tempo sidecar
//!
//! The same pass runs the beat tracker and the downbeat detector, and used to
//! throw both away. [`digest_to_npy_with_tempo`] keeps them as
//! `<song>.tempo.json` beside the `.npy` ([`tempo_path`]): BPM segments, every
//! beat's time, the downbeats, and how much to trust each — the music loops'
//! clock reads a song's bars from it instead of re-hearing them. The `.npy`
//! is byte-for-byte what [`digest_to_npy`] writes; the tempo is a second file,
//! never a seventeenth column (`docs/AUDIO_FEATURES.md`).

use metal_vfs::File;
use std::io::Write;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::resample::{to_mono, Resampler};
use crate::downbeat::BEATS_PER_BAR;
use crate::{Compat, Digester, Row, TickRate, FEATURES};

/// Decodes `path` and digests it **canonically** — one row per 31.25 ms
/// ([`TickRate::CANONICAL`], 32 Hz), 35 columns. The base 8 Hz projection is a
/// pool of these rows ([`pool_to_base`]), never a second digest.
pub fn digest_file(path: &Path) -> Result<Vec<Row>, String> {
    digest_file_with_tempo(path).map(|(rows, _)| rows)
}

/// [`digest_file`] at an explicit level. Used only to produce the corpus's
/// direct base `.8hz.npy` for the one-time re-digest: the live projection is
/// pooled from canonical, but a direct base pass reproduces the pre-V2b base
/// bytes exactly (same window anchor), which is what makes the re-digest
/// verifiable against the old files.
pub fn digest_file_at(path: &Path, rate: TickRate) -> Result<Vec<Row>, String> {
    digest_file_with_tempo_at(path, rate).map(|(rows, _)| rows)
}

/// Pool canonical (fine) rows 4→1 into the base grid, exactly the projection a
/// base machine reads: [`metal_schema::pool_rows`] per group, with the schema's
/// per-column rules (max for `onset`/`hiss`). A trailing group shorter than
/// four is pooled as-is rather than dropped, so no row is lost off the end.
pub fn pool_to_base(rows: &[Row]) -> Vec<Row> {
    rows.chunks(4).map(metal_schema::pool_rows).collect()
}

/// [`digest_file`], plus the tempo map the same pass heard.
pub fn digest_file_with_tempo(path: &Path) -> Result<(Vec<Row>, TempoMap), String> {
    digest_file_with_tempo_at(path, TickRate::CANONICAL)
}

/// [`digest_file_with_tempo`] at an explicit tick level.
pub fn digest_file_with_tempo_at(path: &Path, rate: TickRate) -> Result<(Vec<Row>, TempoMap), String> {
    let samples = decode_to_22050(path)?;
    Ok(digest_samples(&samples, rate))
}

/// Decode `path` the way `librosa.load(path, sr=22050, mono=True)` does:
/// symphonia demux+decode → fold to mono → windowed-sinc resample to
/// [`crate::SR`]. Returns the whole mono 22 050 Hz buffer.
///
/// This is factored out of [`digest_file_with_tempo_at`] so the tone pass
/// ([`digest_file_with_tone`]) can run over the **same** samples the music and
/// voice blocks were measured on, instead of decoding the file a second time.
/// Splitting the loop here does not move a bit: the mono fold and the resampler
/// are both stateful-but-chunk-independent (`resample`'s own tests pin that), so
/// feeding the caller one concatenated buffer is the same as the per-packet
/// pushes the old in-line loop made.
fn decode_to_22050(path: &Path) -> Result<Vec<f32>, String> {
    // Built on the first packet: the rate a track declares in its header is
    // not always there (raw mp3), the decoded buffer's spec always is.
    let mut resampler: Option<Resampler> = None;
    let mut mono = Vec::new();
    let mut resampled = Vec::new();
    let mut out = Vec::new();
    for_each_block(path, |rate, channels, interleaved| {
        mono.clear();
        to_mono(interleaved, channels, &mut mono);
        resampled.clear();
        resampler.get_or_insert_with(|| Resampler::new(rate)).process(&mono, &mut resampled);
        out.extend_from_slice(&resampled);
    })?;
    if resampler.is_none() {
        return Err("the file has no decodable audio".into());
    }
    Ok(out)
}

/// A whole file as interleaved f32 at its own rate, for playback.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

/// Decodes a whole file, unresampled and interleaved: what the native player
/// plays (`metal-dash`'s `call::author::audio_out`).
pub fn decode_pcm(path: &Path) -> Result<Pcm, String> {
    let mut pcm = Pcm { rate: 0, channels: 0, samples: Vec::new() };
    for_each_block(path, |rate, channels, interleaved| {
        pcm.rate = rate;
        pcm.channels = channels;
        pcm.samples.extend_from_slice(interleaved);
    })?;
    if pcm.rate == 0 {
        return Err("the file has no decodable audio".into());
    }
    Ok(pcm)
}

/// Feeds every decoded packet of the file's first audio track to `block` as
/// `(rate, channels, interleaved samples)`. Corrupt packets are skipped.
fn for_each_block(path: &Path, mut block: impl FnMut(u32, usize, &[f32])) -> Result<(), String> {
    let mss = MediaSourceStream::new(open_source(path)?, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("not a readable audio file: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no audio track in the file")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("unsupported codec: {e}"))?;

    let mut interleaved: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("reading the file: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A corrupt frame is skipped, as every player does — one bad
            // packet must not lose the song.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decoding: {e}")),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let buf = interleaved.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if buf.capacity() < decoded.capacity() * channels {
            *buf = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        buf.copy_interleaved_ref(decoded);
        block(spec.rate, channels, buf.samples());
    }
    Ok(())
}

/// Run a whole decoded mono 22 050 Hz buffer through a fresh [`Digester`] at
/// `rate`, close it the way a file must be closed, and return the rows and the
/// tempo map the same pass heard.
fn digest_samples(samples: &[f32], rate: TickRate) -> (Vec<Row>, TempoMap) {
    let mut digester = Digester::new(Compat::Librosa, rate);
    digester.record_tempo();
    digester.push(samples);
    let mut rows = Vec::new();
    while let Some(row) = digester.pop() {
        rows.push(row);
    }
    digester.finish();
    while let Some(row) = digester.pop() {
        rows.push(row);
    }
    let map = TempoMap::from_digester(&digester);
    (rows, map)
}

/// Digest `path` and splice the tone block into columns
/// `MUSIC + VOICE..MUSIC + VOICE + TONE` (29..35), returning the rows and
/// whether a Basic Pitch model was **actually loaded**.
///
/// `model` is normally `Some(Path::new(crate::TONE_MODEL))`; `None`, or a
/// missing/unloadable file, is the supported no-model state (D9/D8): the music
/// and voice columns are still the real digest, the tone block is honest zeros,
/// and `loaded` is `false`. Nothing here fabricates a posteriorgram.
///
/// # Why the tone vector can be shorter than the rows
///
/// The music/voice columns are one row per 32 Hz tick **including** the
/// trailing half-window `Digester::finish` pads on, while
/// [`crate::tone::Tone::reduce_offline`] samples the audio once per 32 Hz tick
/// over the decoded samples alone. The two can differ by at most the one
/// trailing padded row. Where a row has no corresponding tone tick it keeps the
/// zeros [`Digester`] already wrote there; any extra tone ticks past the last
/// row are ignored. Either way the block never borrows a neighbour's value.
#[cfg(feature = "tone")]
pub fn digest_file_with_tone(
    path: &Path,
    model: Option<&Path>,
) -> Result<(Vec<Row>, bool), String> {
    let samples = decode_to_22050(path)?;
    let (mut rows, _) = digest_samples(&samples, TickRate::CANONICAL);

    let mut tone = crate::tone::Tone::new(model);
    let loaded = tone.loaded();
    let cols = tone.reduce_offline(&samples);

    for (i, row) in rows.iter_mut().enumerate() {
        let block = &mut row[crate::MUSIC + crate::VOICE..crate::MUSIC + crate::VOICE + crate::TONE];
        match cols.get(i) {
            Some(c) => block.copy_from_slice(c),
            None => block.fill(0.0),
        }
    }
    Ok((rows, loaded))
}

/// A song's tempo, as heard by one digest — the content of `<song>.tempo.json`.
///
/// Times are seconds from the start of the file. Downbeats use the "1" the
/// detector settled on by the end of the song, applied to every beat: a file
/// is judged with all of it heard, and a DJ set with a bar-shifted section is
/// what the live path and "tap 1" are for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempoMap {
    /// (start time, BPM), in time order; empty when the tracker never locked.
    pub segments: Vec<(f64, f32)>,
    /// Every beat's time.
    pub beats: Vec<f64>,
    /// The beats that are a bar's "1"; empty when no "1" was heard.
    pub downbeats: Vec<f64>,
    /// The tracker's confidence (0..1) at the end of the song.
    pub tempo_confidence: f32,
    /// The downbeat detector's confidence (0..1) at the end of the song.
    pub downbeat_confidence: f32,
}

impl TempoMap {
    fn from_digester(d: &Digester) -> TempoMap {
        let log = d.tempo_log().cloned().unwrap_or_default();
        let one = d.downbeat().phase();
        TempoMap {
            segments: log.segments,
            beats: log.beats.iter().map(|&(t, _)| t).collect(),
            downbeats: log
                .beats
                .iter()
                .filter(|&&(_, b)| one.is_some_and(|p| b.rem_euclid(BEATS_PER_BAR as i64) == p as i64))
                .map(|&(t, _)| t)
                .collect(),
            tempo_confidence: d.tempo().confidence(),
            downbeat_confidence: d.downbeat().confidence(),
        }
    }

    /// The sidecar's JSON, written by hand — this crate keeps to one
    /// dependency, and the shape is a handful of keys of numbers:
    /// `{"version":1,"segments":[[t,bpm],…],"beats":[…],"downbeats":[…],
    /// "tempo_confidence":c,"downbeat_confidence":c}`. Times to the
    /// millisecond, which is finer than a frame.
    pub fn to_json(&self) -> String {
        let list = |v: &[f64]| v.iter().map(|t| format!("{t:.3}")).collect::<Vec<_>>().join(",");
        let segs = self
            .segments
            .iter()
            .map(|(t, b)| format!("[{t:.3},{b:.3}]"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"version\":1,\"segments\":[{segs}],\"beats\":[{}],\"downbeats\":[{}],\"tempo_confidence\":{:.4},\"downbeat_confidence\":{:.4}}}\n",
            list(&self.beats),
            list(&self.downbeats),
            self.tempo_confidence,
            self.downbeat_confidence
        )
    }

    /// Reads what [`TempoMap::to_json`] writes. Not a general JSON parser:
    /// it finds each key and reads the numbers after it, which is all this
    /// file holds.
    pub fn from_json(text: &str) -> Result<TempoMap, String> {
        fn after<'a>(text: &'a str, key: &str) -> Result<&'a str, String> {
            let k = format!("\"{key}\":");
            let at = text.find(&k).ok_or_else(|| format!("tempo.json: no \"{key}\""))?;
            Ok(&text[at + k.len()..])
        }
        /// The numbers in the bracketed list at the start of `s`.
        fn numbers(s: &str) -> Result<Vec<f64>, String> {
            let mut depth = 0;
            let mut end = 0;
            for (i, c) in s.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            s[..end]
                .split(|c: char| c == '[' || c == ']' || c == ',' || c.is_whitespace())
                .filter(|t| !t.is_empty())
                .map(|t| t.parse::<f64>().map_err(|e| format!("tempo.json: {t}: {e}")))
                .collect()
        }
        fn scalar(s: &str) -> Result<f64, String> {
            let t: String = s.chars().take_while(|c| !matches!(c, ',' | '}' | '\n')).collect();
            t.trim().parse().map_err(|e| format!("tempo.json: {t}: {e}"))
        }
        let flat = numbers(after(text, "segments")?)?;
        if flat.len() % 2 != 0 {
            return Err("tempo.json: a segment is not [time, bpm]".into());
        }
        Ok(TempoMap {
            segments: flat.chunks(2).map(|c| (c[0], c[1] as f32)).collect(),
            beats: numbers(after(text, "beats")?)?,
            downbeats: numbers(after(text, "downbeats")?)?,
            tempo_confidence: scalar(after(text, "tempo_confidence")?)? as f32,
            downbeat_confidence: scalar(after(text, "downbeat_confidence")?)? as f32,
        })
    }
}

/// Where the sidecar of the matrix `npy` goes: `<song>.<rate>hz.npy` →
/// `<song>.tempo.json`, `<x>.npy` → `<x>.tempo.json`. The rate is the filename
/// suffix, so a canonical `.32hz.npy` and a base `.8hz.npy` share one tempo
/// file rather than each carrying a second copy.
pub fn tempo_path(npy: &Path) -> std::path::PathBuf {
    let name = npy.file_name().and_then(|n| n.to_str()).unwrap_or("digest");
    let stem = match name.strip_suffix(".npy") {
        Some(body) => match body.rsplit_once('.') {
            Some((stem, tag))
                if tag.ends_with("hz") && tag[..tag.len() - 2].parse::<u32>().is_ok() =>
            {
                stem
            }
            _ => body,
        },
        None => name,
    };
    npy.with_file_name(format!("{stem}.tempo.json"))
}

/// Writes `rows` as a little-endian float32 `(rows, FEATURES)` `.npy` (format
/// 1.0) — the music prefix then the voice block — the file `np.save` writes
/// for the same matrix.
pub fn write_npy(path: &Path, rows: &[Row]) -> std::io::Result<()> {
    let mut header = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': ({}, {}), }}",
        rows.len(),
        FEATURES
    );
    // Magic (6) + version (2) + length (2) + header + '\n', padded to 64.
    let total = 10 + header.len() + 1;
    header.push_str(&" ".repeat((64 - total % 64) % 64));
    header.push('\n');
    let mut out = Vec::with_capacity(10 + header.len() + rows.len() * FEATURES * 4);
    out.extend_from_slice(b"\x93NUMPY\x01\x00");
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    for row in rows {
        for v in row {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut f = File::create(path)?;
    f.write_all(&out)
}

/// [`digest_file`] then [`write_npy`] — `digest_audio.py <in> <out>`.
pub fn digest_to_npy(input: &Path, out: &Path) -> Result<usize, String> {
    let rows = digest_file(input)?;
    write_npy(out, &rows).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(rows.len())
}

/// [`digest_to_npy`], plus the tempo sidecar at [`tempo_path`]`(out)`. The
/// `.npy` is identical to the one [`digest_to_npy`] writes.
pub fn digest_to_npy_with_tempo(input: &Path, out: &Path) -> Result<(usize, TempoMap), String> {
    let (rows, map) = digest_file_with_tempo(input)?;
    write_npy(out, &rows).map_err(|e| format!("{}: {e}", out.display()))?;
    let side = tempo_path(out);
    metal_vfs::write(&side, map.to_json()).map_err(|e| format!("{}: {e}", side.display()))?;
    Ok((rows.len(), map))
}

/// Digest `input` **canonically** and write both levels into `dir`:
/// `dir/<stem>.32hz.npy` (the canonical 35-wide matrix) and
/// `dir/<stem>.8hz.npy` (its pooled base projection), plus one shared tempo
/// sidecar. This is the one path a fresh extract uses; a base machine reads the
/// `.8hz.npy`, a fine one the `.32hz.npy`, and switching between them rewrites
/// neither.
pub fn digest_to_levels_with_tempo(
    input: &Path,
    dir: &Path,
    stem: &str,
) -> Result<(usize, TempoMap), String> {
    let (rows, map) = digest_file_with_tempo(input)?;
    let fine = dir.join(metal_schema::digest_file_name(stem, TickRate::CANONICAL));
    write_npy(&fine, &rows).map_err(|e| format!("{}: {e}", fine.display()))?;
    let base = dir.join(metal_schema::digest_file_name(stem, TickRate::Base));
    write_npy(&base, &pool_to_base(&rows)).map_err(|e| format!("{}: {e}", base.display()))?;
    let side = tempo_path(&fine);
    metal_vfs::write(&side, map.to_json()).map_err(|e| format!("{}: {e}", side.display()))?;
    Ok((rows.len(), map))
}

/// The corpus re-digest: write **both** levels with the base one taken from a
/// direct base pass, so its bytes match the pre-V2b base digest **exactly**.
///
/// The live read path pools canonical on the fly, and that projection is not
/// bit-identical to a direct base pass (a 31.25 ms window holds one or two
/// native frames, not a quarter of the four-to-six a 125 ms window holds, so an
/// unweighted pool of four fine means is not the base window's own mean). This
/// one-time pass sidesteps that: it runs the base Digester directly, exactly
/// the pass V2 shipped, so the music prefix is verifiable against the old file.
/// Writes `<stem>.32hz.npy`, `<stem>.8hz.npy` and one shared tempo sidecar;
/// returns `(fine_rows, base_rows)`.
pub fn digest_to_levels_direct(
    input: &Path,
    dir: &Path,
    stem: &str,
) -> Result<(usize, usize), String> {
    let (fine, map) = digest_file_with_tempo_at(input, TickRate::CANONICAL)?;
    let fine_path = dir.join(metal_schema::digest_file_name(stem, TickRate::CANONICAL));
    write_npy(&fine_path, &fine).map_err(|e| format!("{}: {e}", fine_path.display()))?;
    let base = digest_file_at(input, TickRate::Base)?;
    let base_path = dir.join(metal_schema::digest_file_name(stem, TickRate::Base));
    write_npy(&base_path, &base).map_err(|e| format!("{}: {e}", base_path.display()))?;
    let side = tempo_path(&fine_path);
    metal_vfs::write(&side, map.to_json()).map_err(|e| format!("{}: {e}", side.display()))?;
    Ok((fine.len(), base.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono 16-bit wav of `samples` at `rate`.
    fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }

    /// 30 s of a 120 BPM kick, every fourth one louder: the sidecar lands next
    /// to the `.npy`, round-trips, names ~120 BPM with half-second beats and
    /// downbeats a bar apart on the loud kicks — and the `.npy` is
    /// byte-identical to the plain path's.
    #[test]
    fn the_tempo_sidecar_round_trips_beside_an_identical_npy() {
        let dir = std::env::temp_dir().join(format!("metal-listen-tempo-{}", std::process::id()));
        metal_vfs::create_dir_all(&dir).unwrap();
        let rate = 22050u32;
        let beat = rate as usize / 2;
        let n = rate as usize * 30;
        let samples: Vec<i16> = (0..n)
            .map(|i| {
                let k = i / beat;
                let t = (i % beat) as f32 / rate as f32;
                let amp = if k % 4 == 0 { 1.0 } else { 0.35 };
                let env = (-t * 30.0).exp();
                let tone = (t * 60.0 * std::f32::consts::TAU).sin()
                    + 0.5 * (t * 2500.0 * std::f32::consts::TAU).sin() * (-t * 200.0).exp();
                (amp * env * tone * 16000.0) as i16
            })
            .collect();
        let input = dir.join("kick.wav");
        metal_vfs::write(&input, wav(&samples, rate)).unwrap();

        let plain = dir.join("plain.32hz.npy");
        let with = dir.join("kick.32hz.npy");
        digest_to_npy(&input, &plain).unwrap();
        let (_, map) = digest_to_npy_with_tempo(&input, &with).unwrap();
        assert_eq!(metal_vfs::read(&plain).unwrap(), metal_vfs::read(&with).unwrap(), "the .npy must not change");

        let side = dir.join("kick.tempo.json");
        assert_eq!(tempo_path(&with), side);
        let back = TempoMap::from_json(&metal_vfs::read_to_string(&side).unwrap()).unwrap();
        assert_eq!(back.beats.len(), map.beats.len());
        assert_eq!(back.downbeats.len(), map.downbeats.len());
        assert_eq!(back.segments.len(), map.segments.len());
        for (a, b) in back.beats.iter().zip(&map.beats) {
            assert!((a - b).abs() < 1e-3);
        }

        let bpm = back.segments.last().expect("a kick must lock").1;
        assert!((bpm - 120.0).abs() < 4.0, "bpm {bpm}");
        let gaps: Vec<f64> = back.beats.windows(2).map(|w| w[1] - w[0]).collect();
        let mean = gaps[gaps.len() / 2..].iter().sum::<f64>() / (gaps.len() - gaps.len() / 2) as f64;
        assert!((mean - 0.5).abs() < 0.03, "beat gap {mean}");
        assert!(back.downbeat_confidence > 0.3, "downbeat confidence {}", back.downbeat_confidence);
        assert!(back.downbeats.len() >= 4);
        // Downbeats a bar (2 s) apart and on the louder kicks (t ≡ 0 mod 2 s,
        // give or take the tracker's phase).
        let late = &back.downbeats[back.downbeats.len() / 2..];
        for w in late.windows(2) {
            assert!((w[1] - w[0] - 2.0).abs() < 0.15, "downbeats {w:?}");
        }
        for &t in late {
            let off = (t + 1.0).rem_euclid(2.0) - 1.0;
            assert!(off.abs() < 0.2, "downbeat at {t} is not on a loud kick");
        }
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The shape in a `.npy` header, `(rows, cols)`.
    fn shape_of(path: &Path) -> (usize, usize) {
        let head = metal_vfs::read(path).unwrap();
        let hlen = u16::from_le_bytes([head[8], head[9]]) as usize;
        let text = String::from_utf8_lossy(&head[10..10 + hlen]).to_string();
        let open = text.find('(').unwrap() + 1;
        let close = text[open..].find(')').unwrap() + open;
        let mut it = text[open..close]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().unwrap());
        (it.next().unwrap(), it.next().unwrap())
    }

    /// The canonical extract writes both levels: `.32hz.npy` (35-wide) and the
    /// pooled `.8hz.npy`, sharing one tempo sidecar. The base file is the 4→1
    /// pool of the canonical one, never a second digest.
    #[test]
    fn both_levels_are_written_and_the_base_is_the_pool_of_the_fine() {
        let dir = std::env::temp_dir().join(format!("metal-listen-levels-{}", std::process::id()));
        metal_vfs::create_dir_all(&dir).unwrap();
        let wav = dir.join("song.wav");
        let rate = 22050u32;
        let samples: Vec<i16> = (0..rate * 2)
            .map(|i| {
                let t = i as f32 / rate as f32;
                ((t * 220.0 * std::f32::consts::TAU).sin() * 12000.0) as i16
            })
            .collect();
        metal_vfs::write(&wav, wav_bytes(&samples, rate)).unwrap();

        let (fine_len, _) = digest_to_levels_with_tempo(&wav, &dir, "song").unwrap();
        let fine = dir.join(metal_schema::digest_file_name("song", TickRate::CANONICAL));
        let base = dir.join(metal_schema::digest_file_name("song", TickRate::Base));
        assert!(metal_vfs::is_file(&fine) && metal_vfs::is_file(&base));
        assert_eq!(shape_of(&fine), (fine_len, FEATURES));
        assert_eq!(shape_of(&base), (fine_len.div_ceil(4), FEATURES));
        assert!(metal_vfs::is_file(&dir.join("song.tempo.json")), "one shared sidecar");

        // The projection is the schema's own pooling, not a plain average.
        let rows = digest_file(&wav).unwrap();
        assert_eq!(pool_to_base(&rows)[0], metal_schema::pool_rows(&rows[..4]));
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// A mono 16-bit wav of `samples` at `rate`, in the container the decoder
    /// reads.
    fn wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }

    /// A one-second 440 Hz wav digests **canonically** — ~32 rows a second on
    /// the 32 Hz grid — with A on top of the chroma, and round-trips through
    /// the `.npy` writer with numpy's header.
    #[test]
    fn a_wav_digests_canonically_with_a_on_top() {
        let dir = std::env::temp_dir().join(format!("metal-listen-file-{}", std::process::id()));
        metal_vfs::create_dir_all(&dir).unwrap();
        let wav = dir.join("a440.wav");
        let rate = 44100u32;
        let samples: Vec<i16> = (0..rate)
            .map(|i| ((i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 12000.0) as i16)
            .collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        for s in &samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        metal_vfs::write(&wav, bytes).unwrap();

        let rows = digest_file(&wav).unwrap();
        assert!(
            (31..=34).contains(&rows.len()),
            "one second is ~32 rows on the canonical 32 Hz grid, got {}",
            rows.len()
        );
        let mid = rows[16];
        assert!(mid[0] > 0.1, "loud");
        let a = 4 + 9; // chroma A
        let top = (4..16).max_by(|&i, &j| mid[i].total_cmp(&mid[j])).unwrap();
        assert_eq!(top, a, "440 Hz is an A");

        let npy = dir.join("a440.32hz.npy");
        assert_eq!(digest_to_npy(&wav, &npy).unwrap(), rows.len());
        let head = metal_vfs::read(&npy).unwrap();
        assert_eq!(&head[..6], b"\x93NUMPY");
        let hlen = u16::from_le_bytes([head[8], head[9]]) as usize;
        assert_eq!((10 + hlen) % 64, 0, "numpy aligns the data to 64 bytes");
        let text = String::from_utf8_lossy(&head[10..10 + hlen]);
        assert!(text.contains(&format!("'shape': ({}, {FEATURES})", rows.len())), "{text}");
        assert_eq!(head.len(), 10 + hlen + rows.len() * FEATURES * 4);
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}

/// The bytes of `path` as a symphonia source. Native streams the file; the
/// browser has no file handle that symphonia can seek, so it reads the whole
/// file from the in-memory tree (`metal_vfs`) into a cursor.
#[cfg(not(target_arch = "wasm32"))]
fn open_source(path: &Path) -> Result<Box<dyn symphonia::core::io::MediaSource>, String> {
    let file = metal_vfs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Box::new(file))
}

#[cfg(target_arch = "wasm32")]
fn open_source(path: &Path) -> Result<Box<dyn symphonia::core::io::MediaSource>, String> {
    let bytes = metal_vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Box::new(std::io::Cursor::new(bytes)))
}
