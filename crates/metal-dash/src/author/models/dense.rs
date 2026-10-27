//! Turns a sparse [`Example`] into the dense `(rows, width)` matrix a decoder
//! trains against, and writes it as a `.npy` — the same on-disk shape
//! `data/digest/digest_audio.py` writes for a song's audio digest, so audio
//! and float matrices for one song are the same length by construction:
//! `rows` is read from the song's own `.8hz.npy` header, never guessed from
//! the last authored tick.

use crate::author::models::example::Example;
use std::path::Path;

/// One row per tick, `0..rows`. Each float eased between its own authored
/// keys (see `metal_schema::keys`); before the first authored tick, or after the
/// last, the nearest authored value holds flat.
///
/// This is the ordinary animation-keyframe convention, not the rig's cumulative
/// rotation replay — there is nothing to compose here, only floats to blend,
/// because a governance row is a flat point in a space, not a joint rotated
/// relative to whatever came before it.
///
/// Must agree with `floatsAt` in `page/suite.js` — that is what scrubbing
/// shows, this is what the exported `.npy` contains.
pub fn interpolate(example: &Example, width: usize, rows: usize) -> Vec<Vec<f32>> {
    // Each float eases between its own keys (`metal_schema::keys`), so a
    // keyframe that names other floats does not stall it.
    let tracks = metal_schema::keys::tracks(example.ticks.iter().map(|t| (t.tick, t.floats.as_slice())), width);
    (0..rows)
        .map(|row| metal_schema::keys::row_at(&tracks, row as f32).filter(|r| !r.is_empty()).unwrap_or_else(|| vec![0.0; width]))
        .collect()
}

/// Hand-rolled NPY v1.0 writer — no new dependency. The format: an 8-byte
/// magic + version, a little-endian `u16` header length, an ASCII header
/// dict padded with spaces so `10 + header.len()` lands on a 64-byte
/// boundary (numpy's own alignment rule), then raw little-endian float32 in
/// C order — which a `Vec<Vec<f32>>` already is, row by row.
pub fn write_npy(path: &Path, matrix: &[Vec<f32>], width: usize) -> Result<(), String> {
    let rows = matrix.len();
    let dict = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({rows}, {width}), }}");
    let unpadded = 10 + dict.len() + 1; // +1 for the trailing newline
    let pad = (64 - unpadded % 64) % 64;
    let header = format!("{dict}{}\n", " ".repeat(pad));
    let header_len = u16::try_from(header.len()).map_err(|_| "header too long".to_string())?;

    let mut bytes = Vec::with_capacity(10 + header.len() + rows * width * 4);
    bytes.extend_from_slice(b"\x93NUMPY");
    bytes.push(1); // major version
    bytes.push(0); // minor version
    bytes.extend_from_slice(&header_len.to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    for row in matrix {
        debug_assert_eq!(row.len(), width, "every row must be the declared width");
        for &v in row {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }

    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(path, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author::models::example::Tick;

    fn example(ticks: Vec<(usize, f32)>) -> Example {
        Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: ticks
                .into_iter()
                .map(|(tick, v)| Tick {
                    tick,
                    floats: vec![v],
                })
                .collect(),
        }
    }

    #[test]
    fn a_single_tick_holds_flat_across_every_row() {
        let dense = interpolate(&example(vec![(3, 5.0)]), 1, 10);
        for row in &dense {
            assert_eq!(row[0], 5.0);
        }
    }

    #[test]
    fn two_ticks_ease_between_them_matching_endpoints_and_midpoint() {
        let dense = interpolate(&example(vec![(0, 0.0), (10, 10.0)]), 1, 11);
        assert_eq!(dense[0][0], 0.0);
        assert_eq!(dense[10][0], 10.0);
        assert!(
            (dense[5][0] - 5.0).abs() < 1e-6,
            "smoothstep is symmetric at the midpoint"
        );
        // Easing means the approach to a keyframe is slower than linear: the
        // row a quarter of the way through moves less than a quarter of the span.
        assert!(dense[2][0] < 2.0, "row 2: {}", dense[2][0]);
    }

    #[test]
    fn past_the_last_tick_the_value_holds_rather_than_extrapolates() {
        let dense = interpolate(&example(vec![(0, 1.0), (2, 3.0)]), 1, 6);
        assert_eq!(dense[2][0], 3.0);
        assert_eq!(
            dense[5][0], 3.0,
            "held, not kept climbing at the tick-0..2 slope"
        );
    }

    #[test]
    fn before_the_first_tick_the_value_holds_backward() {
        let dense = interpolate(&example(vec![(4, 9.0)]), 1, 8);
        assert_eq!(dense[0][0], 9.0);
        assert_eq!(dense[3][0], 9.0);
    }

    #[test]
    fn no_authored_ticks_at_all_is_zeros_not_a_panic() {
        let ex = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![],
        };
        let dense = interpolate(&ex, 3, 5);
        assert_eq!(dense.len(), 5);
        assert!(dense.iter().all(|r| r == &[0.0, 0.0, 0.0]));
    }

    #[test]
    fn ticks_out_of_order_on_disk_still_interpolate_correctly() {
        let dense = interpolate(&example(vec![(10, 10.0), (0, 0.0)]), 1, 11);
        assert!(
            (dense[5][0] - 5.0).abs() < 1e-6,
            "smoothstep is symmetric at the midpoint"
        );
    }

    #[test]
    fn the_npy_header_is_bytes_numpy_actually_reads() {
        let dir = std::env::temp_dir().join("fw-corpus-npy");
        let _ = metal_vfs::remove_dir_all(&dir);
        let path = dir.join(metal_schema::digest_file_name("t", metal_schema::TickRate::Base));
        let matrix = vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]];
        write_npy(&path, &matrix, 3).expect("write");

        let bytes = metal_vfs::read(&path).expect("read back");
        assert_eq!(&bytes[0..6], b"\x93NUMPY");
        assert_eq!(bytes[6], 1, "major version");
        assert_eq!(bytes[7], 0, "minor version");
        let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        assert_eq!((10 + header_len) % 64, 0, "aligned to a 64-byte boundary");
        let header = String::from_utf8_lossy(&bytes[10..10 + header_len]);
        assert!(header.contains("'shape': (2, 3)"));
        assert!(header.contains("'descr': '<f4'"));
        assert!(header.ends_with('\n'));

        let data = &bytes[10 + header_len..];
        assert_eq!(data.len(), 2 * 3 * 4);
        let first = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        assert_eq!(first, 1.0);
        let last4 = &data[data.len() - 4..];
        let last = f32::from_le_bytes([last4[0], last4[1], last4[2], last4[3]]);
        assert_eq!(last, 6.0);
    }
}
