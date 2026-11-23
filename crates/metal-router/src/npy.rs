//! The smallest `.npy` reader that reads what `digest_audio.py` writes.
//!
//! A plain float32 array is a magic number, a header and then the floats. A
//! dependency to parse that would be more code than this is — the same call
//! `metal-listen`'s reference test made, and this is that reader promoted out of a
//! test because the router needs it at run time: training reads every song's
//! matrix, and the studio's router lane draws one.
//!
//! It only handles what the pipeline actually produces — little-endian float32,
//! two dimensions — and says so rather than guessing when handed anything else.

use std::path::Path;

use crate::features::COLUMNS;

/// The width of a digest row through the voice block — the music prefix plus
/// the voice block, before the tone block was appended. It is the second width
/// the pipeline wrote (and what a matrix digested before the tone block still
/// is), so [`load`] and [`has_voice`] must keep accepting it and read its tone
/// columns as zero.
pub const FULL_VOICE: usize = metal_schema::MUSIC + metal_schema::VOICE;

/// Reads `<song>.8hz.npy` as rows of [`COLUMNS`] floats.
///
/// Accepts the three widths the pipeline has ever written: the script's
/// [`metal_schema::MUSIC`] (16), the pre-tone Rust digester's [`FULL_VOICE`] (29)
/// and the current [`metal_schema::FEATURES`] (35). A row is always [`COLUMNS`]
/// wide; any column the file does not carry stays zero, so a 16-wide matrix
/// loads with the voice and tone blocks at zero and a 29-wide one with the tone
/// block at zero. [`has_voice`] and [`has_tone`] report which blocks are
/// actually present, so the timeline can say "not digested for voice" instead
/// of showing a silent singer. Anything else is an error rather than something
/// to pad or truncate.
pub fn load(path: &Path) -> Result<Vec<[f32; COLUMNS]>, String> {
    let bytes = metal_vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{} is not a .npy", path.display()));
    }
    let (header_len, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        _ => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
    };
    if start + header_len > bytes.len() {
        return Err(format!("{}: header runs past the end", path.display()));
    }
    let header = String::from_utf8_lossy(&bytes[start..start + header_len]).to_string();
    if !(header.contains("'<f4'") || header.contains("\"<f4\"")) {
        return Err(format!("{} is not little-endian float32", path.display()));
    }
    if header.contains("'fortran_order': True") {
        // The digester writes C order. Column-major here would mean the matrix
        // came from somewhere else, and reading it as C order would silently
        // scramble every row — worse than refusing.
        return Err(format!("{} is column-major; the digest is not", path.display()));
    }

    let open = header.find('(').ok_or("no shape in the header")?;
    let close = header[open..].find(')').ok_or("no shape in the header")? + open;
    let shape: Vec<usize> = header[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if shape.len() != 2 {
        return Err(format!("{}: expected a 2-D matrix, got {shape:?}", path.display()));
    }
    let width = shape[1];
    if width != metal_schema::MUSIC && width != FULL_VOICE && width != metal_schema::FEATURES {
        return Err(format!(
            "{}: {width} columns, but a digest row is {}, {} or {}",
            path.display(),
            metal_schema::MUSIC,
            FULL_VOICE,
            metal_schema::FEATURES
        ));
    }

    let body = &bytes[start + header_len..];
    let want = shape[0] * width * 4;
    if body.len() < want {
        return Err(format!("{}: {} bytes short", path.display(), want - body.len()));
    }

    let mut out = Vec::with_capacity(shape[0]);
    for r in 0..shape[0] {
        let mut row = [0.0f32; COLUMNS];
        for (c, slot) in row.iter_mut().enumerate().take(width) {
            let at = (r * width + c) * 4;
            *slot = f32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
        }
        out.push(row);
    }
    Ok(out)
}

/// Whether a stem's only digest is a base file — no canonical `.32hz.npy`
/// exists, so a fine read holds it as steps. This is the "base-only" mark the
/// timeline/tick page shows, the one honest loss of the migration (an 8 Hz
/// digest with no audio left to re-digest).
pub fn is_base_only(dir: &Path, stem: &str) -> bool {
    !metal_vfs::is_file(&canonical_path(dir, stem))
        && (metal_vfs::is_file(&base_path(dir, stem)) || metal_vfs::is_file(&dir.join(format!("{stem}.npy"))))
}

/// The canonical (32 Hz) matrix path for a stem under `dir`.
pub fn canonical_path(dir: &Path, stem: &str) -> std::path::PathBuf {
    dir.join(metal_schema::digest_file_name(stem, metal_schema::TickRate::CANONICAL))
}

/// The pooled base (8 Hz) projection path for a stem under `dir`.
pub fn base_path(dir: &Path, stem: &str) -> std::path::PathBuf {
    dir.join(metal_schema::digest_file_name(stem, metal_schema::TickRate::Base))
}

/// The file to read for `stem` at an explicit tick level, preferring the
/// matching one: a fine reader wants `.32hz.npy`, a base reader wants the
/// `.8hz.npy` projection, and either falls back to the other then to the
/// legacy `<stem>.npy`. The rate is the filename suffix — there is no in-file
/// rate — so this is the one place that resolves which grid a file is on.
pub fn song_path(dir: &Path, stem: &str, active: metal_schema::TickRate) -> Option<std::path::PathBuf> {
    let canon = canonical_path(dir, stem);
    let base = base_path(dir, stem);
    let legacy = dir.join(format!("{stem}.npy"));
    let order = if active == metal_schema::TickRate::Fine {
        [canon, base, legacy]
    } else {
        [base, canon, legacy]
    };
    order.into_iter().find(|p| metal_vfs::is_file(&p))
}

/// [`load`] at an explicit level. A base read of a canonical file pools 4→1 (the
/// schema's per-column rules); a fine read of a base-only file holds each row as
/// four steps — the honest "base-only" expansion, never an invented
/// interpolation.
pub fn load_song(
    dir: &Path,
    stem: &str,
    active: metal_schema::TickRate,
) -> Result<Vec<[f32; COLUMNS]>, String> {
    let path = song_path(dir, stem, active).ok_or_else(|| {
        format!(
            "no digest for {stem:?} under {} (tried a canonical, base or legacy .npy)",
            dir.display()
        )
    })?;
    let rows = load(&path)?;
    let on_fine = metal_schema::digest_rate_of(
        path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    ) == Some(metal_schema::TickRate::Fine);
    match (active, on_fine) {
        // Already the right grid.
        (metal_schema::TickRate::Fine, true) | (metal_schema::TickRate::Base, false) => Ok(rows),
        // Fine file read at base: pool 4→1.
        (metal_schema::TickRate::Base, true) => {
            Ok(rows.chunks(4).map(metal_schema::pool_rows).collect())
        }
        // Base-only file read at fine: hold each row as four steps.
        (metal_schema::TickRate::Fine, false) => {
            Ok(rows.iter().flat_map(|r| std::iter::repeat(*r).take(4)).collect())
        }
    }
}

/// Rows a stem's digest has on the `active` grid, from the chosen file's header
/// alone — scaling a base-only file `×4` (held steps) and a canonical file
/// `÷4` (pooled) so the count is always in the active level's ticks. `None`
/// when no digest exists.
pub fn ticks_at(dir: &Path, stem: &str, active: metal_schema::TickRate) -> Option<usize> {
    let path = song_path(dir, stem, active)?;
    let n = ticks(&path)?;
    let on_fine = metal_schema::digest_rate_of(path.file_name().and_then(|f| f.to_str())?)
        == Some(metal_schema::TickRate::Fine);
    Some(match (active, on_fine) {
        (metal_schema::TickRate::Base, true) => n.div_ceil(4),
        (metal_schema::TickRate::Fine, false) => n * 4,
        _ => n,
    })
}

/// A `.npy`'s column count from its header alone, or `None` when the file is
/// missing or the first 256 bytes do not hold a readable shape. The one place
/// [`has_voice`] and [`has_tone`] read the width, so the two can never parse it
/// differently.
fn header_width(path: &Path) -> Option<usize> {
    let head = metal_vfs::read(path).ok()?;
    let head = head.get(..256.min(head.len()))?;
    let text = String::from_utf8_lossy(head);
    let open = text.find('(')? + 1;
    let close = text[open..].find(')')? + open;
    text[open..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .next_back()?
        .parse()
        .ok()
}

/// Whether a `.npy`'s header says its rows carry the voice block — `None` when
/// the file is missing or not a readable matrix with a known width. Voice is
/// present for the 29- and 35-wide rows and absent for the 16-wide script
/// matrix; the timeline uses this to say "not digested for voice" for a 16-wide
/// song.
pub fn has_voice(path: &Path) -> Option<bool> {
    match header_width(path)? {
        w if w == metal_schema::MUSIC => Some(false),
        w if w == FULL_VOICE || w == metal_schema::FEATURES => Some(true),
        _ => None,
    }
}

/// Whether a `.npy`'s header says its rows carry the tone block — `None` when
/// the file is missing or not a readable matrix with a known width. Only the
/// current 35-wide row carries it; the 16- and 29-wide rows predate the tone
/// block and read as zeros there, exactly as a 16-wide row reads the voice
/// block as zeros.
pub fn has_tone(path: &Path) -> Option<bool> {
    match header_width(path)? {
        w if w == metal_schema::FEATURES => Some(true),
        w if w == metal_schema::MUSIC || w == FULL_VOICE => Some(false),
        _ => None,
    }
}

/// Rows in a matrix, from the header alone.
///
/// Reading a four-minute song's whole matrix to find out how long it is costs
/// a megabyte for a number that is in the first hundred bytes; the studio's
/// listing asks this of every song in the folder on every request.
pub fn ticks(path: &Path) -> Option<usize> {
    let head = metal_vfs::read(path).ok()?;
    let head = head.get(..256.min(head.len()))?;
    let text = String::from_utf8_lossy(head);
    let at = text.find("'shape': (")? + 10;
    text[at..].split(',').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header `numpy.save` writes, and the one `dense::write_npy` in
    /// `fw-studio` writes — this reader has to accept both, and they are the
    /// same shape.
    fn write(path: &Path, rows: usize, cols: usize) {
        let header = format!(
            "{{'descr': '<f4', 'fortran_order': False, 'shape': ({rows}, {cols}), }}"
        );
        let mut pad = header.into_bytes();
        while (10 + pad.len() + 1) % 64 != 0 {
            pad.push(b' ');
        }
        pad.push(b'\n');
        let mut out = Vec::new();
        out.extend_from_slice(b"\x93NUMPY\x01\x00");
        out.extend_from_slice(&(pad.len() as u16).to_le_bytes());
        out.extend_from_slice(&pad);
        for r in 0..rows {
            for c in 0..cols {
                out.extend_from_slice(&((r * cols + c) as f32).to_le_bytes());
            }
        }
        metal_vfs::create_dir_all(path.parent().unwrap()).unwrap();
        metal_vfs::write(path, out).unwrap();
    }

    fn tmp(stem: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join("metal-router-npy")
            .join(metal_schema::digest_file_name(stem, metal_schema::TickRate::Base))
    }

    #[test]
    fn it_reads_a_matrix_the_pipeline_would_write() {
        let p = tmp("ok");
        write(&p, 5, COLUMNS);
        let m = load(&p).expect("loads");
        assert_eq!(m.len(), 5);
        assert_eq!(m[0][0], 0.0);
        assert_eq!(m[1][0], COLUMNS as f32);
        assert_eq!(m[4][COLUMNS - 1], (5 * COLUMNS - 1) as f32);
        assert_eq!(ticks(&p), Some(5));
    }

    #[test]
    fn old_widths_load_with_the_new_blocks_zero() {
        // Three widths have shipped. Each must load at the current width with
        // the columns it does not carry left at zero, and the presence helpers
        // must say which blocks are real rather than the row silently growing
        // meaning.
        let p = tmp("script");
        write(&p, 5, metal_schema::MUSIC);
        let m = load(&p).expect("a 16-wide matrix still loads");
        assert_eq!(m.len(), 5);
        for (r, row) in m.iter().enumerate() {
            assert!(
                row[metal_schema::MUSIC..].iter().all(|v| *v == 0.0),
                "row {r}'s voice+tone blocks are not zero"
            );
        }
        assert_eq!(has_voice(&p), Some(false), "16 wide means no voice");
        assert_eq!(has_tone(&p), Some(false), "16 wide means no tone");

        // The pre-tone digester's 29-wide row: voice is real, tone is zero.
        let q = tmp("digest");
        write(&q, 5, FULL_VOICE);
        let voice = load(&q).expect("a 29-wide matrix still loads");
        assert_eq!(voice.len(), 5);
        assert_eq!(voice[4][FULL_VOICE - 1], (5 * FULL_VOICE - 1) as f32);
        assert!(
            voice[4][FULL_VOICE..].iter().all(|v| *v == 0.0),
            "a 29-wide row's tone block is not zero"
        );
        assert_eq!(has_voice(&q), Some(true), "29 wide means voice present");
        assert_eq!(has_tone(&q), Some(false), "29 wide means no tone");

        // The current 35-wide row carries both.
        let r = tmp("tone");
        write(&r, 5, metal_schema::FEATURES);
        let full = load(&r).expect("a 35-wide matrix loads");
        assert_eq!(full[4][metal_schema::FEATURES - 1], (5 * metal_schema::FEATURES - 1) as f32);
        assert_eq!(has_voice(&r), Some(true), "35 wide means voice present");
        assert_eq!(has_tone(&r), Some(true), "35 wide means tone present");
    }

    #[test]
    fn a_matrix_of_the_wrong_width_is_refused_rather_than_reshaped() {
        // Padding or truncating here would hand the model rows whose columns
        // mean something else, and nothing downstream could tell.
        let p = tmp("narrow");
        write(&p, 5, 8);
        let e = load(&p).expect_err("should refuse");
        assert!(e.contains("8 columns"), "{e}");
    }

    #[test]
    fn base_only_is_a_base_file_with_no_canonical_sibling() {
        let dir = std::env::temp_dir().join("metal-router-baseonly");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        write(&base_path(&dir, "b"), 3, COLUMNS);
        assert!(is_base_only(&dir, "b"), "only a base file is base-only");
        write(&canonical_path(&dir, "b"), 9, COLUMNS);
        assert!(!is_base_only(&dir, "b"), "a canonical sibling means not base-only");
        assert!(!is_base_only(&dir, "nope"), "no digest at all is not base-only");
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn something_that_is_not_a_npy_says_so() {
        let p = tmp("nope");
        metal_vfs::create_dir_all(p.parent().unwrap()).unwrap();
        metal_vfs::write(&p, b"not a matrix at all").unwrap();
        assert!(load(&p).is_err());
        assert_eq!(ticks(&p), None);
    }

    /// Both levels live on disk at once; reading either level is a pure read
    /// and switching base↔fine rewrites neither. This is the lossless-switch
    /// promise: nothing stored is edited by a change of machine level.
    #[test]
    fn switching_fine_base_fine_changes_no_stored_byte() {
        let dir = std::env::temp_dir().join("metal-router-switch");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let canon = canonical_path(&dir, "s");
        write(&canon, 8, COLUMNS);
        let base = base_path(&dir, "s");
        write(&base, 2, COLUMNS);
        let (cb, bb) = (metal_vfs::read(&canon).unwrap(), metal_vfs::read(&base).unwrap());

        let at_base = load_song(&dir, "s", metal_schema::TickRate::Base).expect("base read");
        let at_fine = load_song(&dir, "s", metal_schema::TickRate::Fine).expect("fine read");
        assert_eq!(at_base.len(), 2, "base reads the projection");
        assert_eq!(at_fine.len(), 8, "fine reads the canonical matrix");

        assert_eq!(metal_vfs::read(&canon).unwrap(), cb, "the canonical bytes moved");
        assert_eq!(metal_vfs::read(&base).unwrap(), bb, "the base bytes moved");
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// A base read of a canonical-only file pools 4→1; a fine read of a
    /// base-only file (no audio left to re-digest) holds each row as four
    /// steps — the honest `base-only` expansion, never an invented curve.
    #[test]
    fn a_base_read_pools_and_a_fine_read_of_base_only_steps() {
        let dir = std::env::temp_dir().join("metal-router-pool");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        let canon = canonical_path(&dir, "a");
        write(&canon, 8, COLUMNS);
        assert_eq!(load_song(&dir, "a", metal_schema::TickRate::Base).unwrap().len(), 2);

        let base = base_path(&dir, "b");
        write(&base, 3, COLUMNS);
        let fine = load_song(&dir, "b", metal_schema::TickRate::Fine).unwrap();
        assert_eq!(fine.len(), 12, "each base row is held as four fine steps");
        assert_eq!(fine[0], fine[3], "the same row across its four steps");
        assert_eq!(fine[4], fine[7]);
        assert_ne!(fine[0], fine[4], "the next base row is a different step");
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
