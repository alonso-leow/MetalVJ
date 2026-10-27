//! Does this crate agree with librosa?
//!
//! `docs/AUDIO_FEATURES.md` open item 4 says there are two digesters and that
//! the rule forbids it — "digest one file both ways and compare the matrices
//! column by column. Until then, treat the lanes as a *view* of the music and
//! the `.npy` as the truth." This is that comparison, for the third digester,
//! run before it was allowed to drive anything.
//!
//! It needs a reference bundle, which is not committed — several megabytes of
//! float arrays nobody can review is not what a repository is for. Generate one
//! and point the test at it:
//!
//! ```text
//! data/digest/.venv/bin/python scripts/librosa_reference.py \
//!     "data/audio/<a song>.mp3" /tmp/ref 30
//! FW_LISTEN_REFERENCE=/tmp/ref cargo test -p metal-listen --test reference -- --nocapture
//! ```
//!
//! Without `FW_LISTEN_REFERENCE` every test here skips rather than fails: a
//! machine with no bundle is not a machine with a broken digester, and a test
//! that cries wolf gets ignored exactly when it matters.

use std::path::PathBuf;

use metal_listen::{Compat, Digester, TickRate, COLUMNS, FEATURES, MUSIC, N_CHROMA};

// --- the smallest .npy reader that does this job ---------------------------
//
// A plain float32 array is a magic number, a header and then the floats. A
// dependency to parse that would be more code than this is.

struct Npy {
    shape: Vec<usize>,
    data: Vec<f32>,
}

impl Npy {
    fn load(path: &PathBuf) -> Npy {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(&bytes[..6], b"\x93NUMPY", "{} is not a .npy", path.display());
        let (header_len, start) = match bytes[6] {
            1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
            _ => (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12,
            ),
        };
        let header = String::from_utf8_lossy(&bytes[start..start + header_len]).to_string();
        assert!(
            header.contains("'<f4'") || header.contains("\"<f4\""),
            "{} is not little-endian float32 — regenerate the bundle",
            path.display()
        );
        // librosa hands back chroma as a transposed view, so numpy saves it
        // column-major. Transposing it here is three lines; regenerating the
        // bundle with `np.ascontiguousarray` everywhere would work too and
        // would break the next time somebody dumps an array without it.
        let fortran = header.contains("'fortran_order': True");

        let open = header.find('(').expect("no shape in header");
        let close = header[open..].find(')').expect("no shape in header") + open;
        let shape: Vec<usize> = header[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().expect("shape entry"))
            .collect();

        let body = &bytes[start + header_len..];
        let data: Vec<f32> = body
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let data = if fortran && shape.len() == 2 {
            let (rows, cols) = (shape[0], shape[1]);
            let mut c_order = vec![0.0f32; data.len()];
            for r in 0..rows {
                for c in 0..cols {
                    c_order[r * cols + c] = data[c * rows + r];
                }
            }
            c_order
        } else {
            data
        };

        Npy { shape, data }
    }

    /// Row `r` of a 2-D array.
    fn row(&self, r: usize) -> &[f32] {
        let w = self.shape[1];
        &self.data[r * w..(r + 1) * w]
    }
}

fn bundle() -> Option<String> {
    match std::env::var("FW_LISTEN_REFERENCE") {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("skipping: set FW_LISTEN_REFERENCE to a bundle prefix (see module docs)");
            None
        }
    }
}

fn part(prefix: &str, name: &str) -> Npy {
    Npy::load(&PathBuf::from(format!("{prefix}_{name}.npy")))
}

// --- how close is close ----------------------------------------------------

fn pearson(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let ma = a.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mb = b.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mut num = 0.0;
    let mut da = 0.0;
    let mut db = 0.0;
    for i in 0..n {
        let (x, y) = (a[i] as f64 - ma, b[i] as f64 - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 {
        return 1.0; // two constants agree perfectly; nothing to correlate
    }
    num / (da.sqrt() * db.sqrt())
}

/// Largest absolute difference, and the same relative to the reference's own
/// spread — a 40 Hz error means nothing on a column that ranges over 4000.
fn error(a: &[f32], b: &[f32]) -> (f64, f64) {
    let n = a.len().min(b.len());
    let max = (0..n).fold(0.0f64, |m, i| m.max((a[i] - b[i]).abs() as f64));
    let lo = b[..n].iter().cloned().fold(f32::INFINITY, f32::min) as f64;
    let hi = b[..n].iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
    let span = (hi - lo).max(1e-12);
    (max, max / span)
}

fn run(prefix: &str) -> (Digester, Vec<[f32; FEATURES]>) {
    let y = part(prefix, "y");
    let mut d = Digester::new(Compat::Librosa, TickRate::Base);
    d.trace_native();
    d.push(&y.data);
    d.finish();
    let rows: Vec<[f32; FEATURES]> = std::iter::from_fn(|| d.pop()).collect();
    (d, rows)
}

// --- the tests -------------------------------------------------------------

#[test]
fn the_filterbanks_are_the_ones_librosa_builds() {
    let Some(prefix) = bundle() else { return };

    let mel_ref = part(&prefix, "melfb");
    let mel = metal_listen::filters::mel(metal_listen::SR, metal_listen::N_FFT, metal_listen::N_MELS);
    assert_eq!((mel.rows, mel.bins), (mel_ref.shape[0], mel_ref.shape[1]));
    let mut worst = 0.0f64;
    for r in 0..mel.rows {
        for (k, &want) in mel_ref.row(r).iter().enumerate() {
            worst = worst.max((mel.get(r, k) - want).abs() as f64);
        }
    }
    println!("mel filterbank    worst element error {worst:.3e}");
    assert!(worst < 1e-6, "the mel bank is not librosa's (worst {worst:.3e})");

    let ch_ref = part(&prefix, "chromafb");
    let ch = metal_listen::filters::chroma(metal_listen::SR, metal_listen::N_FFT, N_CHROMA);
    assert_eq!((ch.rows, ch.bins), (ch_ref.shape[0], ch_ref.shape[1]));
    let mut worst = 0.0f64;
    for r in 0..ch.rows {
        for (k, &want) in ch_ref.row(r).iter().enumerate() {
            worst = worst.max((ch.get(r, k) - want).abs() as f64);
        }
    }
    println!("chroma filterbank worst element error {worst:.3e}");
    // Looser than mel and deliberately so: this bank is built at tuning 0.0
    // and librosa's default estimates the tuning off the file. The gap is the
    // divergence `lib.rs` names, measured rather than assumed.
    assert!(worst < 1e-5, "the chroma bank is not librosa's (worst {worst:.3e})");
}

#[test]
fn every_native_feature_matches_frame_by_frame() {
    let Some(prefix) = bundle() else { return };
    let (d, _) = run(&prefix);
    let native = d.native();

    let rms = part(&prefix, "rms");
    let bandwidth = part(&prefix, "bandwidth");
    let centroid = part(&prefix, "centroid");
    let onset = part(&prefix, "onset");
    let chroma = part(&prefix, "chroma");

    println!(
        "frames: rust {} vs librosa {} (chroma {:?})",
        native.len(),
        rms.data.len(),
        chroma.shape
    );
    assert!(
        (native.len() as i64 - rms.data.len() as i64).abs() <= 1,
        "frame counts differ by more than the trailing pad: {} vs {}",
        native.len(),
        rms.data.len()
    );

    let n = native.len().min(rms.data.len());
    let mine: Vec<f32> = native[..n].iter().map(|f| f.rms).collect();
    let (max, rel) = error(&mine, &rms.data);
    let r = pearson(&mine, &rms.data);
    println!("rms        r={r:.9}  max={max:.3e}  rel={rel:.3e}");
    assert!(r > 0.99999 && rel < 1e-4, "rms diverged");

    let mine: Vec<f32> = native[..n].iter().map(|f| f.centroid).collect();
    let (max, rel) = error(&mine, &centroid.data);
    let r = pearson(&mine, &centroid.data);
    println!("centroid   r={r:.9}  max={max:.3e} Hz  rel={rel:.3e}");
    assert!(r > 0.99999 && rel < 1e-4, "centroid diverged");

    let mine: Vec<f32> = native[..n].iter().map(|f| f.bandwidth).collect();
    let (max, rel) = error(&mine, &bandwidth.data);
    let r = pearson(&mine, &bandwidth.data);
    println!("bandwidth  r={r:.9}  max={max:.3e} Hz  rel={rel:.3e}");
    assert!(r > 0.99999 && rel < 1e-4, "bandwidth diverged");

    // Chroma arrives transposed: librosa gives (12, frames).
    let frames = chroma.shape[1];
    let mut worst_r = 1.0f64;
    for c in 0..N_CHROMA {
        let theirs: Vec<f32> = (0..frames.min(n)).map(|t| chroma.data[c * frames + t]).collect();
        let mine: Vec<f32> = native[..theirs.len()].iter().map(|f| f.chroma[c]).collect();
        let r = pearson(&mine, &theirs);
        let (_, rel) = error(&mine, &theirs);
        worst_r = worst_r.min(r);
        println!("chroma {:<3}  r={r:.9}  rel={rel:.3e}", COLUMNS[4 + c]);
    }
    // Tuning again: at A440 this is near-exact, and on a record cut sharp it
    // will not be. The correlation is what has to hold.
    assert!(worst_r > 0.999, "a chroma class diverged (worst r={worst_r:.6})");

    let mine: Vec<f32> = native[..n].iter().map(|f| f.onset).collect();
    let (max, rel) = error(&mine, &onset.data);
    let r = pearson(&mine, &onset.data);
    println!("onset      r={r:.9}  max={max:.3e}  rel={rel:.3e}");
    // The running decibel floor lives in this column and nowhere else.
    assert!(r > 0.999, "onset diverged (r={r:.6})");
}

/// Synthetic pulse trains are not music, and passing on them proved less than
/// it looked: the tracker locked on a metronome and stayed silent through an
/// actual record. This is the test that had to exist.
#[test]
fn the_beat_clock_locks_onto_a_real_song() {
    let Some(prefix) = bundle() else { return };
    let y = part(&prefix, "y");
    let mut d = Digester::new(Compat::Live, TickRate::Base);
    d.push(&y.data);
    let t = d.tempo();
    println!(
        "after {:.0} s: bpm={:?} beats={:?} confidence={:.4}",
        y.data.len() as f32 / metal_listen::SR as f32,
        t.bpm(),
        t.beats(),
        t.confidence()
    );
    let bpm = t.bpm().expect("thirty seconds of a dance track and it never locked");
    assert!(
        (60.0..=200.0).contains(&bpm),
        "{bpm} BPM is not a tempo anybody plays"
    );
    let beats = t.beats().expect("locked but counting nothing");
    assert!(beats > 1.0, "locked at {bpm} BPM but only counted {beats} beats");
    // The operator reads this to decide whether to trust the beat light, so a
    // confident lock has to actually say so. It read 0.0000 on a track it had
    // correctly found the tempo of, which is worse than useless.
    assert!(
        t.confidence() > 0.05,
        "locked at {bpm} BPM but reported {:.4} confidence",
        t.confidence()
    );
}

#[test]
fn the_pooled_matrix_is_the_one_the_corpus_would_have_got() {
    let Some(prefix) = bundle() else { return };
    let (_, rows) = run(&prefix);
    let want = part(&prefix, "matrix");

    println!("rows: rust {} vs librosa {}", rows.len(), want.shape[0]);
    assert!(
        (rows.len() as i64 - want.shape[0] as i64).abs() <= 1,
        "row counts differ by more than the trailing all-zero window"
    );

    // The corpus has no voice block, so only the music prefix is the script's.
    assert_eq!(want.shape[1], MUSIC, "the corpus matrix is the music prefix");
    let n = rows.len().min(want.shape[0]);
    let mut worst = ("", 0.0f64);
    for c in 0..MUSIC {
        let mine: Vec<f32> = rows[..n].iter().map(|r| r[c]).collect();
        let theirs: Vec<f32> = (0..n).map(|t| want.row(t)[c]).collect();
        let r = pearson(&mine, &theirs);
        let (max, rel) = error(&mine, &theirs);
        println!("[{c:2}] {:<10} r={r:.9}  max={max:.4e}  rel={rel:.3e}", COLUMNS[c]);
        if 1.0 - r > worst.1 {
            worst = (COLUMNS[c], 1.0 - r);
        }
        assert!(r > 0.999, "column {} diverged (r={r:.6})", COLUMNS[c]);
    }
    println!("\nworst column: {} (1-r = {:.3e})", worst.0, worst.1);
}
