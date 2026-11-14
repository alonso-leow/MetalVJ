//! the loops design notes T17: the live tempo + downbeat path on real songs, and
//! (V6) the shipped voice analyser on the same live path.
//!
//! Feeds a file through [`Digester`] the way `capture.rs` feeds a sound card
//! — `Compat::Live`, mono at the device rate, resampled to 22 050 Hz in
//! 1024-frame chunks (a typical cpal buffer). Without `--voice` it logs, per
//! song: tracked BPM over time, re-locks, the heard "1" + confidence over
//! time, against the sidecar `digest_file_with_tempo` writes and against the
//! strongest low-frequency (< ~150 Hz) onsets folded by beat position.
//!
//! With `--voice` it runs the real, shipped `metal_listen::voice` analyser (the
//! one `Digester` owns and `metal-perform::face` consumes) and prints **presence
//! and style per second**, then scores that per-second presence against
//! `data/voice-labels/*.toml` where a label exists. A label with no `voice`
//! key is *pending* and is reported as such, never scored.
//!
//! ```text
//! cargo run --release -p metal-listen --features decode --example live_from_file -- [--trace] <audio>…
//! cargo run --release -p metal-listen --features decode --example live_from_file -- --voice [--labels <dir>] <audio>…
//! ```
//!
//! The expected BPM is the file's ID3 `TBPM` (via `ffprobe` when present) or
//! a number in the filename. Nothing is written; audio is only read.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use metal_listen::file::digest_file_with_tempo;
use metal_listen::resample::{to_mono, Resampler};
use metal_listen::voice;
use metal_listen::{Compat, Digester, Row, TickRate, HOP, MUSIC, SR};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Frames per chunk handed to the digester, at the file's own rate.
const CHUNK: usize = 1024;

/// The file as mono at its own rate.
fn decode(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no track")?;
    let id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    let (mut out, mut rate, mut mono) = (Vec::new(), 0u32, Vec::new());
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(_)) | Err(Error::ResetRequired) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else { continue };
        let spec = *decoded.spec();
        rate = spec.rate;
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        mono.clear();
        to_mono(buf.samples(), spec.channels.count().max(1), &mut mono);
        out.extend_from_slice(&mono);
    }
    Ok((out, rate))
}

/// ID3 TBPM via ffprobe, else the first 2–3 digit number 60..200 in the name.
fn expected_bpm(path: &Path) -> Option<f32> {
    let tag = Command::new("ffprobe")
        .args(["-v", "quiet", "-show_entries", "format_tags=TBPM,bpm", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().and_then(|l| l.trim().parse::<f32>().ok()));
    tag.or_else(|| {
        let name = path.file_stem()?.to_str()?;
        name.split(|c: char| !c.is_ascii_digit())
            .filter_map(|n| n.parse::<f32>().ok())
            .find(|b| (60.0..=200.0).contains(b))
    })
}

/// Low-band onset envelope at 22 050 Hz, one value per HOP: two one-pole
/// low-passes at ~150 Hz, energy per hop, positive differences.
fn low_onsets(x: &[f32]) -> Vec<f32> {
    let a = (-2.0 * std::f32::consts::PI * 150.0 / SR as f32).exp();
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    let mut energy = Vec::with_capacity(x.len() / HOP + 1);
    let mut acc = 0.0f32;
    for (i, &s) in x.iter().enumerate() {
        y1 = (1.0 - a) * s + a * y1;
        y2 = (1.0 - a) * y1 + a * y2;
        acc += y2 * y2;
        if (i + 1) % HOP == 0 {
            energy.push((acc / HOP as f32).sqrt());
            acc = 0.0;
        }
    }
    let mut on = vec![0.0; energy.len()];
    for i in 1..energy.len() {
        on[i] = (energy[i] - energy[i - 1]).max(0.0);
    }
    on
}

/// Mean low-band onset at beats of each position mod 4 (`beats`: (time, index)).
fn low_fold(on: &[f32], beats: &[(f64, i64)]) -> [f32; 4] {
    let fps = SR as f64 / HOP as f64;
    let (mut sum, mut n) = ([0.0f32; 4], [0u32; 4]);
    for &(t, b) in beats {
        let c = (t * fps).round() as i64;
        let peak = (c - 3..=c + 3).filter_map(|i| on.get(i.max(0) as usize)).fold(0.0f32, |m, &v| m.max(v));
        let p = b.rem_euclid(4) as usize;
        sum[p] += peak;
        n[p] += 1;
    }
    std::array::from_fn(|i| if n[i] == 0 { 0.0 } else { sum[i] / n[i] as f32 })
}

fn octave_ok(bpm: f32, want: f32) -> bool {
    [0.5f32, 1.0, 2.0].iter().any(|k| ((bpm - want * k) / (want * k)).abs() < 0.03)
}

/// The default mode: the the loops design notes T17 tempo/downbeat table.
fn tempo_main(files: &[String], trace: bool) {
    println!("| song | expected | live BPM (end) | sidecar BPM | lock s | re-locks | live 1 (end) | 1 stable from s | phase flips | conf med | sidecar 1 agrees | low-onset 1 |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    for arg in files {
        let path = Path::new(arg);
        let name: String = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").chars().take(48).collect();
        let want = expected_bpm(path);
        let (mono, rate) = match decode(path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{arg}: {e}");
                continue;
            }
        };
        // The live path: Compat::Live, chunked like a sound card.
        let mut d = Digester::new(Compat::Live, TickRate::Base);
        d.record_tempo();
        let mut rs = Resampler::new(rate);
        let mut at = Vec::new();
        let mut all22 = Vec::with_capacity(mono.len() / 2);
        let mut lock_s: Option<f64> = None;
        let mut first_lock_s: Option<f64> = None;
        let mut phase_log: Vec<(f64, Option<u8>, f32)> = Vec::new();
        let mut confs = Vec::new();
        let mut next_log = 0.0f64;
        for chunk in mono.chunks(CHUNK) {
            at.clear();
            rs.process(chunk, &mut at);
            all22.extend_from_slice(&at);
            d.push(&at);
            while d.pop().is_some() {}
            let t = d.frames() as f64 * HOP as f64 / SR as f64;
            let bpm = d.tempo().bpm();
            if first_lock_s.is_none() && bpm.is_some() {
                first_lock_s = Some(t);
            }
            if lock_s.is_none() && bpm.zip(want).is_some_and(|(b, w)| octave_ok(b, w)) {
                lock_s = Some(t);
            }
            if t >= next_log {
                next_log += 1.0;
                let heard = d.downbeat().heard();
                phase_log.push((t, heard.map(|h| h.0), heard.map_or(0.0, |h| h.1)));
                if let Some(h) = heard {
                    confs.push(h.1);
                }
                if trace && (t as u64) % 10 == 0 {
                    println!(
                        "  {name} t={t:6.1} bpm={:?} conf={:.2} relocks={} beats={:?} one={:?}",
                        bpm.map(|b| (b * 10.0).round() / 10.0),
                        d.tempo().confidence(),
                        d.tempo().changes(),
                        d.tempo().beats().map(|b| b.floor()),
                        heard
                    );
                }
            }
        }
        let live_bpm = d.tempo().bpm();
        let live_one = d.downbeat().heard();
        // When the "1" last changed (stable from), and how often it flipped.
        let mut flips = 0;
        let mut stable_from = None;
        let mut prev: Option<u8> = None;
        for &(t, p, _) in &phase_log {
            if p != prev {
                if prev.is_some() {
                    flips += 1;
                }
                stable_from = Some(t);
                prev = p;
            }
        }
        confs.sort_by(f32::total_cmp);
        let conf_med = confs.get(confs.len() / 2).copied().unwrap_or(0.0);

        // The file path: what the sidecar would say.
        let (side_bpm, side_agree) = match digest_file_with_tempo(path) {
            Ok((_, map)) => {
                let mut bpms: Vec<f32> = map.segments.iter().map(|s| s.1).collect();
                bpms.sort_by(f32::total_cmp);
                let med = bpms.get(bpms.len() / 2).copied();
                // Live "1"s (end phase on the live count) vs sidecar downbeats, last half of the song.
                let half = mono.len() as f64 / rate as f64 / 2.0;
                let agree = live_one.zip(d.tempo_log()).map(|((p, _), log)| {
                    let ones: Vec<f64> =
                        log.beats.iter().filter(|(t, b)| *t > half && b.rem_euclid(4) == p as i64).map(|b| b.0).collect();
                    let hit = ones.iter().filter(|t| map.downbeats.iter().any(|d| (d - **t).abs() < 0.07)).count();
                    if ones.is_empty() { 0.0 } else { hit as f32 / ones.len() as f32 }
                });
                (med, agree)
            }
            Err(e) => {
                eprintln!("{arg}: sidecar: {e}");
                (None, None)
            }
        };
        // Ear-equivalent: which position carries the strongest low-band onsets.
        let low = d.tempo_log().map(|log| low_fold(&low_onsets(&all22), &log.beats));
        let low_one = low.map(|f| {
            let best = (0..4).max_by(|&a, &b| f[a].total_cmp(&f[b])).unwrap();
            let mut s = f;
            s.sort_by(f32::total_cmp);
            (best, if s[3] > 0.0 { (s[3] - s[2]) / s[3] } else { 0.0 })
        });
        let f1 = |v: Option<f32>| v.map_or("—".into(), |b| format!("{b:.1}"));
        let fs = |v: Option<f64>| v.map_or("—".into(), |b| format!("{b:.1}"));
        println!(
            "| {name} | {} | {} | {} | {} (first {}) | {} | {} | {} | {flips} | {conf_med:.2} | {} | {} |",
            f1(want),
            f1(live_bpm),
            f1(side_bpm),
            fs(lock_s),
            fs(first_lock_s),
            d.tempo().changes(),
            live_one.map_or("—".into(), |(p, c)| format!("{p} ({c:.2})")),
            fs(stable_from),
            side_agree.map_or("—".into(), |a| format!("{:.0}%", a * 100.0)),
            match (low_one, live_one) {
                (Some((b, m)), Some((p, _))) => format!("{b} (margin {:.0}%){}", m * 100.0, if b as u8 == p { " =" } else { " ≠" }),
                (Some((b, m)), None) => format!("{b} (margin {:.0}%)", m * 100.0),
                _ => "—".into(),
            },
        );
    }
}

// --- V6: the shipped voice analyser, per second, on the live path -----------

/// A second is "voiced" when the mean `voice` column of its rows reaches this.
/// The column is already the per-tick fraction of frames that decided voice, so
/// this is the per-second reading of the same decision, at V0's 0.3 threshold.
const PRESENCE_SECOND: f32 = 0.3;

/// The four style columns, in [`voice`]'s order, and their names.
const STYLES: [usize; 4] = [voice::SING, voice::RAP, voice::SPEAK, voice::WHISPER];
const STYLE_NAMES: [&str; 4] = ["sing", "rap", "speak", "whisper"];

/// A hand label read from `data/voice-labels/<stem>.toml`. `voice == None` is a
/// template with no `voice` key: *pending*, not all-silent (see V0).
struct Labels {
    voice: Option<Vec<(f64, f64)>>,
}

fn read_labels(path: &Path) -> Result<Labels, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: toml::Value = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    let voice = match v.get("voice") {
        None => None,
        Some(toml::Value::Array(a)) => {
            let mut spans = Vec::new();
            for e in a {
                let arr = e.as_array().ok_or("voice span is not [start, end]")?;
                if arr.len() != 2 {
                    return Err("voice span is not [start, end]".into());
                }
                spans.push((
                    arr[0].as_float().ok_or("span start is not a number")?,
                    arr[1].as_float().ok_or("span end is not a number")?,
                ));
            }
            Some(spans)
        }
        Some(_) => return Err("voice is not an array".into()),
    };
    Ok(Labels { voice })
}

fn truth_at(spans: &[(f64, f64)], second: usize) -> bool {
    let mid = second as f64 + 0.5;
    spans.iter().any(|&(a, b)| mid >= a && mid < b)
}

fn ratio(num: u64, den: u64) -> String {
    if den == 0 {
        "n/a".into()
    } else {
        format!("{:.2}", num as f32 / den as f32)
    }
}

/// The outcome of scoring one file's per-second presence against its label.
enum Scored {
    /// A label exists but its `voice` key is commented out — pending.
    Pending,
    /// A real label: (tp, fp, fn, tn) over its seconds.
    Scored { tp: u64, fp: u64, fneg: u64, tn: u64 },
}

fn stem_of(arg: &str) -> String {
    Path::new(arg).file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut trace = false;
    let mut voice_mode = false;
    let mut labels_dir = PathBuf::from("data/voice-labels");
    let mut files: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--trace" => trace = true,
            "--voice" => voice_mode = true,
            "--labels" => {
                i += 1;
                labels_dir = PathBuf::from(args.get(i).cloned().unwrap_or_default());
            }
            other => files.push(other.to_string()),
        }
        i += 1;
    }
    if voice_mode {
        voice_main(&files, &labels_dir);
    } else {
        tempo_main(&files, trace);
    }
}

fn voice_main(files: &[String], labels_dir: &Path) {
    println!("# live_from_file --voice: the shipped metal_listen::voice analyser on the live path.");
    println!("# Compat::Live; mono at the device rate resampled to {SR} Hz in {CHUNK}-frame chunks; TickRate::Base (8 rows/s).");
    println!("# A second is voiced when the mean of its rows' `voice` column >= {PRESENCE_SECOND}.");
    println!("# style is the argmax of the four soft scores summed over a voiced second.");
    let mut total = (0u64, 0u64, 0u64, 0u64);
    let mut pending: Vec<String> = Vec::new();
    let mut scored = 0u32;
    for arg in files {
        match voice_one(Path::new(arg), labels_dir) {
            Some(Scored::Pending) => pending.push(stem_of(arg)),
            Some(Scored::Scored { tp, fp, fneg, tn }) => {
                total.0 += tp;
                total.1 += fp;
                total.2 += fneg;
                total.3 += tn;
                scored += 1;
            }
            None => {}
        }
    }
    let (tp, fp, fneg, tn) = total;
    println!("\n=== precision / recall (labels that exist) ===");
    println!(
        "  scored files {scored}   TP {tp}  FP {fp}  FN {fneg}  TN {tn}   precision {}   recall {}",
        ratio(tp, tp + fp),
        ratio(tp, tp + fneg)
    );
    if !pending.is_empty() {
        println!("  LABELS PENDING (template, no `voice` key): {}", pending.join(", "));
    }
    if scored == 0 {
        println!("  no scored labels (the rest are pending) — nothing to report against");
    }
}

fn voice_one(path: &Path, labels_dir: &Path) -> Option<Scored> {
    let name = stem_of(&path.to_string_lossy());
    let (mono, rate) = match decode(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{name}: {e}");
            return None;
        }
    };
    // The live path: Compat::Live, chunked like a sound card.
    let mut d = Digester::new(Compat::Live, TickRate::Base);
    let mut rs = Resampler::new(rate);
    let mut at = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    for chunk in mono.chunks(CHUNK) {
        at.clear();
        rs.process(chunk, &mut at);
        d.push(&at);
        while let Some(r) = d.pop() {
            rows.push(r);
        }
    }
    // End of file only: close the trailing partial tick so the tail is not lost.
    d.finish();
    while let Some(r) = d.pop() {
        rows.push(r);
    }

    let per_second = TickRate::Base.hz() as usize;
    let seconds = rows.len().div_ceil(per_second);
    println!(
        "\n=== {name} ({:.1}s, {} rows, {seconds}s) ===",
        rows.len() as f32 / per_second as f32,
        rows.len()
    );
    println!("  sec  voice  level  pitch  style        sing    rap  speak   whis");

    let mut pred: Vec<bool> = Vec::with_capacity(seconds);
    let mut style_seconds: [u32; 4] = [0; 4];
    for (s, group) in rows.chunks(per_second).enumerate() {
        let n = group.len() as f32;
        let voice = group.iter().map(|r| r[MUSIC + voice::VOICE]).sum::<f32>() / n;
        let level = group.iter().map(|r| r[MUSIC + voice::VOICE_LEVEL]).sum::<f32>() / n;
        let (mut pitch_sum, mut pitch_n) = (0.0f32, 0u32);
        for r in group {
            let p = r[MUSIC + voice::PITCH];
            if p > 0.0 {
                pitch_sum += p;
                pitch_n += 1;
            }
        }
        let pitch = if pitch_n > 0 { pitch_sum / pitch_n as f32 } else { 0.0 };
        let sums: Vec<f32> =
            STYLES.iter().map(|&c| group.iter().map(|r| r[MUSIC + c]).sum::<f32>()).collect();
        let voiced = voice >= PRESENCE_SECOND;
        pred.push(voiced);
        let best = (0..4).max_by(|&a, &b| sums[a].total_cmp(&sums[b])).unwrap();
        let style = if voiced {
            style_seconds[best] += 1;
            STYLE_NAMES[best]
        } else {
            "—"
        };
        println!(
            "  {s:03}  {voice:.2}   {level:.2}   {pitch:.2}   {style:<8}  {:.2}  {:.2}  {:.2}  {:.2}",
            sums[0], sums[1], sums[2], sums[3]
        );
    }
    let voiced_seconds = pred.iter().filter(|&&b| b).count();
    let share = if seconds > 0 { voiced_seconds as f32 / seconds as f32 } else { 0.0 };
    println!(
        "  voiced seconds {voiced_seconds}/{seconds} ({share:.2})  styles  sing {}  rap {}  speak {}  whisper {}",
        style_seconds[0], style_seconds[1], style_seconds[2], style_seconds[3]
    );

    // Score against the label, if there is one and it is not a template.
    let label_path = labels_dir.join(format!("{name}.toml"));
    if !label_path.exists() {
        println!("  label: none at {}", label_path.display());
        return None;
    }
    let labels = match read_labels(&label_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{name}: {e}");
            return None;
        }
    };
    let Some(spans) = labels.voice.as_ref() else {
        println!("  label: PENDING (template has no `voice` key) — not scored");
        return Some(Scored::Pending);
    };
    let label_seconds = spans.iter().map(|&(_, b)| b.ceil() as usize).max().unwrap_or(0);
    let n = pred.len().max(label_seconds);
    let (mut tp, mut fp, mut fneg, mut tn) = (0u64, 0u64, 0u64, 0u64);
    for s in 0..n {
        let p = pred.get(s).copied().unwrap_or(false);
        match (truth_at(spans, s), p) {
            (true, true) => tp += 1,
            (false, true) => fp += 1,
            (true, false) => fneg += 1,
            (false, false) => tn += 1,
        }
    }
    println!(
        "  label: TP {tp}  FP {fp}  FN {fneg}  TN {tn}   precision {}   recall {}",
        ratio(tp, tp + fp),
        ratio(tp, tp + fneg)
    );
    Some(Scored::Scored { tp, fp, fneg, tn })
}
