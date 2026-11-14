//! QOI (the "Quite OK Image" format), RGBA only, for frames between machines.
//!
//! A remote layer's picture is a 1280×720 RGBA buffer. JPEG spends 2–6 ms
//! decoding it and ~16 ms encoding it on the peer, and it is lossy, so the
//! same layer drawn locally and drawn remotely would not match. QOI is
//! lossless, a few-line codec, and encodes a frame of synthetic or
//! flat-coloured content far faster than JPEG. The format is the spec from
//! qoiformat.org (header `qoif`, 14 bytes; 8-byte end marker).

const MAGIC: &[u8; 4] = b"qoif";
/// The multi-strip container: `qois`, `w`, `h`, strip count (all big-endian),
/// then each strip as `rows`, payload length, and the payload. It exists so a
/// frame can be encoded in independent strips — one per core — and still
/// travel as the one QOI body a single `GET` carries. See [`encode_strips`].
const SMAGIC: &[u8; 4] = b"qois";
const END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];
const OP_INDEX: u8 = 0x00;
const OP_DIFF: u8 = 0x40;
const OP_LUMA: u8 = 0x80;
const OP_RUN: u8 = 0xc0;
const OP_RGB: u8 = 0xfe;
const OP_RGBA: u8 = 0xff;
const MASK2: u8 = 0xc0;

#[inline]
fn hash(px: [u8; 4]) -> usize {
    (px[0] as usize * 3 + px[1] as usize * 5 + px[2] as usize * 7 + px[3] as usize * 11) % 64
}

/// Encodes `rgba` (`width * height * 4` bytes) to a QOI file, sRGB, RGBA.
pub fn encode(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let n = (width as usize) * (height as usize);
    debug_assert_eq!(rgba.len(), n * 4, "qoi: buffer does not match the size");
    // Worst case is five bytes a pixel (RGBA literal), plus header and end.
    let mut out = vec![0u8; 14 + n * 5 + END.len()];
    out[..4].copy_from_slice(MAGIC);
    out[4..8].copy_from_slice(&width.to_be_bytes());
    out[8..12].copy_from_slice(&height.to_be_bytes());
    out[12] = 4;
    out[13] = 0;
    let at = encode_ops(rgba, &mut out, 14, false);
    out[at..at + END.len()].copy_from_slice(&END);
    out.truncate(at + END.len());
    out
}

/// Writes `rows` (a whole number of pixel rows, `chunks_exact(4)`) as QOI ops
/// into `out` from `at`, and returns the new `at`. `index`/`prev` start fresh,
/// so a band is a self-contained op stream; with `force_first` its first pixel
/// is written as an `OP_RGBA` literal regardless of state, so a band can be
/// decoded on its own from the same fresh state ([`encode_strips`]).
fn encode_ops(rows: &[u8], out: &mut [u8], mut at: usize, force_first: bool) -> usize {
    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0, 0, 255];
    let mut run: u8 = 0;
    let mut first = force_first;
    for chunk in rows.chunks_exact(4) {
        let px = [chunk[0], chunk[1], chunk[2], chunk[3]];
        if first {
            out[at..at + 5].copy_from_slice(&[OP_RGBA, px[0], px[1], px[2], px[3]]);
            at += 5;
            index[hash(px)] = px;
            prev = px;
            first = false;
            continue;
        }
        if px == prev {
            run += 1;
            if run == 62 {
                out[at] = OP_RUN | (run - 1);
                at += 1;
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out[at] = OP_RUN | (run - 1);
            at += 1;
            run = 0;
        }
        let h = hash(px);
        if index[h] == px {
            out[at] = OP_INDEX | h as u8;
            at += 1;
        } else {
            index[h] = px;
            if px[3] == prev[3] {
                let vr = px[0].wrapping_sub(prev[0]) as i8 as i32;
                let vg = px[1].wrapping_sub(prev[1]) as i8 as i32;
                let vb = px[2].wrapping_sub(prev[2]) as i8 as i32;
                if (-2..2).contains(&vr) && (-2..2).contains(&vg) && (-2..2).contains(&vb) {
                    out[at] = OP_DIFF | (((vr + 2) << 4) | ((vg + 2) << 2) | (vb + 2)) as u8;
                    at += 1;
                } else {
                    let (vg_r, vg_b) = (vr - vg, vb - vg);
                    if (-32..32).contains(&vg) && (-8..8).contains(&vg_r) && (-8..8).contains(&vg_b) {
                        out[at] = OP_LUMA | (vg + 32) as u8;
                        out[at + 1] = (((vg_r + 8) << 4) | (vg_b + 8)) as u8;
                        at += 2;
                    } else {
                        out[at..at + 4].copy_from_slice(&[OP_RGB, px[0], px[1], px[2]]);
                        at += 4;
                    }
                }
            } else {
                out[at..at + 5].copy_from_slice(&[OP_RGBA, px[0], px[1], px[2], px[3]]);
                at += 5;
            }
        }
        prev = px;
    }
    if run > 0 {
        out[at] = OP_RUN | (run - 1);
        at += 1;
    }
    at
}

/// The row count of each of `strips` bands over `height` rows: even, with the
/// first `height % strips` bands one row taller. Never zero for `strips <= height`.
fn strip_rows(height: usize, strips: usize) -> Vec<usize> {
    let strips = strips.max(1).min(height.max(1));
    let (base, rem) = (height / strips, height % strips);
    (0..strips).map(|i| base + usize::from(i < rem)).collect()
}

/// Assembles the [`SMAGIC`] container of `(rows, payload)` bands. Determinism
/// is the caller's: the bands are written in the order given.
fn container(width: u32, height: u32, bands: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + bands.iter().map(|(_, p)| 8 + p.len()).sum::<usize>());
    out.extend_from_slice(SMAGIC);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&(bands.len() as u32).to_be_bytes());
    for (rows, payload) in bands {
        out.extend_from_slice(&rows.to_be_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&payload);
    }
    out
}

/// Encodes `rgba` as `strips` independent horizontal bands, each starting with
/// an `OP_RGBA` literal and carrying its own index/prev, into one [`SMAGIC`]
/// container (`strips` is clamped to `1..=height`). Strips are what make the
/// encode parallel — see [`encode_parallel`] — and [`decode_strips`] reads the
/// container back losslessly.
pub fn encode_strips(rgba: &[u8], width: u32, height: u32, strips: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    debug_assert_eq!(rgba.len(), w * h * 4, "qoi: buffer does not match the size");
    let counts = strip_rows(h, strips as usize);
    let mut bands = Vec::with_capacity(counts.len());
    let mut row = 0usize;
    for rows in counts {
        let band = &rgba[row * w * 4..(row + rows) * w * 4];
        let mut payload = vec![0u8; band.len() / 4 * 5];
        let at = encode_ops(band, &mut payload, 0, true);
        payload.truncate(at);
        bands.push((rows as u32, payload));
        row += rows;
    }
    container(width, height, bands)
}

/// [`encode_strips`] with the bands encoded on threads, so the calling thread
/// does not sit through the whole 720p encode. The container is byte-for-byte
/// what the serial form makes; one strip writes a plain `qoif` file and never
/// spawns, so a short frame's body is the one a peer has always read.
pub fn encode_parallel(rgba: &[u8], width: u32, height: u32, strips: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    debug_assert_eq!(rgba.len(), w * h * 4, "qoi: buffer does not match the size");
    let counts = strip_rows(h, strips as usize);
    if counts.len() <= 1 {
        return encode(rgba, width, height);
    }
    // Each band's byte span up front, so the threads borrow disjoint slices of
    // `rgba` rather than moving it.
    let spans: Vec<(usize, usize)> = {
        let mut spans = Vec::with_capacity(counts.len());
        let mut row = 0usize;
        for rows in &counts {
            spans.push((row * w * 4, rows * w * 4));
            row += rows;
        }
        spans
    };
    let mut payloads: Vec<Vec<u8>> = vec![Vec::new(); counts.len()];
    std::thread::scope(|scope| {
        for (span, payload) in spans.iter().zip(payloads.iter_mut()) {
            let (start, len) = *span;
            let band = &rgba[start..start + len];
            scope.spawn(move || {
                let mut buf = vec![0u8; len / 4 * 5];
                let at = encode_ops(band, &mut buf, 0, true);
                buf.truncate(at);
                *payload = buf;
            });
        }
    });
    container(width, height, counts.iter().map(|r| *r as u32).zip(payloads).collect())
}

/// Decodes a QOI file to `(width, height, RGBA)`. `None` for anything that is
/// not a well-formed QOI file of 3 or 4 channels. A [`SMAGIC`] strip container
/// is read through [`decode_strips`], so either body a peer sends works here.
pub fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if bytes.starts_with(SMAGIC) {
        return decode_strips(bytes);
    }
    if bytes.len() < 14 + END.len() || &bytes[..4] != MAGIC {
        return None;
    }
    let width = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[8..12].try_into().ok()?);
    let channels = bytes[12];
    if !(3..=4).contains(&channels) || width == 0 || height == 0 {
        return None;
    }
    let n = width as usize * height as usize;
    let mut out = vec![0u8; n * 4];
    decode_stream(&bytes[14..bytes.len() - END.len()], n, &mut out)?;
    Some((width, height, out))
}

/// One op stream into `out` as `n` pixels, from fresh index/prev — the body of
/// [`decode`] and of each strip [`decode_strips`] reads. `None` when the ops
/// run out before `out` is full.
fn decode_stream(data: &[u8], n: usize, out: &mut [u8]) -> Option<()> {
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut at = 0usize;
    let mut pos = 0usize;
    while pos < n * 4 {
        let b = *data.get(at)?;
        at += 1;
        if b == OP_RGB {
            px[0..3].copy_from_slice(data.get(at..at + 3)?);
            at += 3;
        } else if b == OP_RGBA {
            px.copy_from_slice(data.get(at..at + 4)?);
            at += 4;
        } else if b & MASK2 == OP_INDEX {
            px = index[(b & 0x3f) as usize];
        } else if b & MASK2 == OP_DIFF {
            px[0] = px[0].wrapping_add(((b >> 4) & 3).wrapping_sub(2));
            px[1] = px[1].wrapping_add(((b >> 2) & 3).wrapping_sub(2));
            px[2] = px[2].wrapping_add((b & 3).wrapping_sub(2));
        } else if b & MASK2 == OP_LUMA {
            let b2 = *data.get(at)?;
            at += 1;
            let vg = (b & 0x3f).wrapping_sub(32);
            let vg_r = (b2 >> 4).wrapping_sub(8).wrapping_add(vg);
            let vg_b = (b2 & 0x0f).wrapping_sub(8).wrapping_add(vg);
            px[0] = px[0].wrapping_add(vg_r);
            px[1] = px[1].wrapping_add(vg);
            px[2] = px[2].wrapping_add(vg_b);
        } else {
            // OP_RUN: this pixel and `run` more repeat the previous one.
            let run = (b & 0x3f) as usize + 1;
            if pos + run * 4 > n * 4 {
                return None;
            }
            for slot in out[pos..pos + run * 4].chunks_exact_mut(4) {
                slot.copy_from_slice(&px);
            }
            pos += run * 4;
            continue;
        }
        index[hash(px)] = px;
        out[pos..pos + 4].copy_from_slice(&px);
        pos += 4;
    }
    Some(())
}

/// Decodes a [`SMAGIC`] strip container: `None` for a foreign or truncated
/// body, or one whose bands do not add up to `height` rows.
pub fn decode_strips(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if bytes.len() < 16 || !bytes.starts_with(SMAGIC) {
        return None;
    }
    let width = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[8..12].try_into().ok()?);
    let strips = u32::from_be_bytes(bytes[12..16].try_into().ok()?) as usize;
    if width == 0 || height == 0 || strips == 0 {
        return None;
    }
    let w = width as usize;
    let mut out = vec![0u8; w * height as usize * 4];
    let mut at = 16usize;
    let mut row = 0usize;
    for _ in 0..strips {
        let rows = u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        let len = u32::from_be_bytes(bytes.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        at += 8;
        let payload = bytes.get(at..at + len)?;
        at += len;
        if rows == 0 || row + rows > height as usize {
            return None;
        }
        let pixels = rows * w;
        let start = row * w * 4;
        decode_stream(payload, pixels, &mut out[start..start + pixels * 4])?;
        row += rows;
    }
    if row != height as usize {
        return None;
    }
    Some((width, height, out))
}

/// The RGB bytes of an RGBA buffer (alpha dropped), the shape a remote
/// picture is composed from.
pub fn rgb_of(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        let mut s: u32 = 0x1234_5678;
        for y in 0..h {
            for x in 0..w {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (s >> 16) as u8;
                // Gradients, flat runs, a noisy band and an alpha edge.
                let (r, g, b) = match (x / 40 + y / 30) % 4 {
                    0 => (x as u8, y as u8, 7),
                    1 => (9, 9, 9),
                    2 => (noise, noise / 2, noise.wrapping_add(3)),
                    _ => (200, 10, 90),
                };
                let a = if x < w / 3 { 255 } else { 128 };
                v.extend_from_slice(&[r, g, b, a]);
            }
        }
        v
    }

    #[test]
    fn round_trips_every_op_losslessly() {
        for (w, h) in [(1, 1), (7, 5), (64, 48), (161, 93)] {
            let rgba = pattern(w, h);
            let bytes = encode(&rgba, w, h);
            let (dw, dh, back) = decode(&bytes).expect("decodes");
            assert_eq!((dw, dh), (w, h));
            assert_eq!(back, rgba, "{w}x{h} round-trips exactly");
        }
    }

    #[test]
    fn a_flat_frame_is_a_run_and_compresses_hard() {
        let rgba = vec![0x40u8; 320 * 180 * 4];
        let bytes = encode(&rgba, 320, 180);
        assert!(bytes.len() < 2000, "flat frame encodes to {} bytes", bytes.len());
        assert_eq!(decode(&bytes).unwrap().2, rgba);
    }

    #[test]
    fn rejects_truncated_and_foreign_input() {
        assert!(decode(b"qoif").is_none());
        assert!(decode(&[0xff; 64]).is_none());
        let mut bytes = encode(&pattern(8, 8), 8, 8);
        bytes.truncate(bytes.len() - 20);
        assert!(decode(&bytes).is_none());
    }

    /// Every strip count round-trips exactly, and `decode` reads the container
    /// the same as `decode_strips`: a strip boundary is not a visible one.
    #[test]
    fn strips_round_trip_every_op_losslessly() {
        for (w, h) in [(1, 1), (7, 5), (64, 48), (161, 93)] {
            let rgba = pattern(w, h);
            for strips in [1u32, 2, 3, 4, 7, 8, 16, 1000] {
                let bytes = encode_strips(&rgba, w, h, strips);
                assert!(bytes.starts_with(SMAGIC), "a strip body is a container");
                let (dw, dh, back) = decode(&bytes).expect("decodes");
                assert_eq!((dw, dh), (w, h), "{w}x{h} / {strips} strips");
                assert_eq!(back, rgba, "{w}x{h} / {strips} strips round-trip exactly");
                assert_eq!(decode_strips(&bytes).unwrap().2, rgba, "decode_strips agrees");
            }
        }
    }

    /// Each strip begins with an `OP_RGBA` literal, so it can be decoded on its
    /// own from the fresh index/prev a strip decoder starts with.
    #[test]
    fn each_strip_starts_with_an_rgba_literal() {
        let bytes = encode_strips(&pattern(8, 8), 8, 8, 4);
        let strips = u32::from_be_bytes(bytes[12..16].try_into().unwrap()) as usize;
        assert_eq!(strips, 4, "the header names the strips asked for");
        let mut at = 16usize;
        for _ in 0..strips {
            let rows = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
            let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            at += 8;
            assert!(rows > 0, "a strip has rows");
            assert_eq!(bytes[at], OP_RGBA, "the strip's first op is a literal");
            at += len;
        }
        assert_eq!(at, bytes.len(), "the strips fill the body");
    }

    /// The threaded encode is byte-for-byte the serial one, for every split;
    /// one strip is a plain `qoif` file, not a container.
    #[test]
    fn parallel_strip_encode_matches_the_serial_one() {
        let rgba = pattern(128, 96);
        assert_eq!(encode_parallel(&rgba, 128, 96, 1), encode(&rgba, 128, 96), "one strip is a plain file");
        for strips in [2u32, 3, 4, 8, 12] {
            assert_eq!(
                encode_parallel(&rgba, 128, 96, strips),
                encode_strips(&rgba, 128, 96, strips),
                "{strips} strips"
            );
        }
    }

    /// A container that is short, has a zero strip, or whose bands do not add
    /// up to the height is refused rather than half-decoded.
    #[test]
    fn rejects_a_malformed_strip_container() {
        let bytes = encode_strips(&pattern(8, 8), 8, 8, 4);
        assert!(decode_strips(b"qois").is_none(), "too short");
        assert!(decode_strips(&bytes[..bytes.len() - 4]).is_none(), "truncated");
        let mut zero = bytes.clone();
        zero[12..16].copy_from_slice(&0u32.to_be_bytes());
        assert!(decode_strips(&zero).is_none(), "no strips");
        let mut tall = bytes.clone();
        tall[8..12].copy_from_slice(&64u32.to_be_bytes());
        assert!(decode_strips(&tall).is_none(), "bands do not reach the height");
    }

    /// `cargo test --release -p metal-dash qoi::tests::bench_720p -- --ignored --nocapture`
    /// prints encode and decode time per 1280×720 frame, per format, for a
    /// shader-like gradient and for the noisy worst case. Numbers in DOCS.md.
    #[test]
    #[ignore]
    fn bench_720p() {
        let smooth: Vec<u8> = (0..720u32)
            .flat_map(|y| (0..1280u32).flat_map(move |x| [(x / 5) as u8, (y / 4) as u8, ((x + y) / 9) as u8, 255]))
            .collect();
        bench_one("gradient", &smooth);
        bench_one("noisy", &pattern(1280, 720));
    }

    /// Best of five rounds of `runs` calls each, ms per call. On a box shared
    /// with other builds the mean is the load; the fastest round is the code.
    fn time_ms(runs: u32, mut f: impl FnMut()) -> f64 {
        (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                for _ in 0..runs {
                    f();
                }
                t.elapsed().as_secs_f64() * 1000.0 / runs as f64
            })
            .fold(f64::INFINITY, f64::min)
    }

    fn bench_one(name: &str, rgba: &[u8]) {
        let (w, h) = (1280u32, 720u32);
        let qoi = encode(rgba, w, h);
        let enc = time_ms(20, || {
            std::hint::black_box(encode(rgba, w, h));
        });
        let dec = time_ms(20, || {
            std::hint::black_box(decode(&qoi).unwrap());
        });
        assert_eq!(decode(&qoi).unwrap().2, rgba);
        println!("{name:8} qoi   encode {enc:6.2} ms  decode {dec:6.2} ms  {:>8} bytes", qoi.len());

        // The same frame in independent strips, encoded one per core on
        // threads (`encode_parallel`, what `call::render::Codec::Qoi` uses).
        let strips = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
        let qoi_strips = encode_parallel(rgba, w, h, strips);
        let enc = time_ms(20, || {
            std::hint::black_box(encode_parallel(rgba, w, h, strips));
        });
        let dec = time_ms(20, || {
            std::hint::black_box(decode(&qoi_strips).unwrap());
        });
        assert_eq!(decode(&qoi_strips).unwrap().2, rgba);
        println!("{name:8} qoix  encode {enc:6.2} ms  decode {dec:6.2} ms  {:>8} bytes  ({strips} strips)", qoi_strips.len());

        let jpg = crate::paint::encode_jpeg_q(rgba, w, h, 82);
        let enc = time_ms(20, || {
            std::hint::black_box(crate::paint::encode_jpeg_q(rgba, w, h, 82));
        });
        let dec = time_ms(20, || {
            std::hint::black_box(crate::shell::frames::decode_jpeg_rgb(&jpg).unwrap());
        });
        println!("{name:8} jpeg  encode {enc:6.2} ms  decode {dec:6.2} ms  {:>8} bytes (q82, lossy)", jpg.len());

        let enc = time_ms(20, || {
            std::hint::black_box(rgba.to_vec());
        });
        println!("{name:8} rgba  encode {enc:6.2} ms  decode   0.00 ms  {:>8} bytes", rgba.len());
    }
}
