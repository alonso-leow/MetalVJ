//! 2026-10-01: HAP — footage the GPU reads as it is stored.
//!
//! ```text
//!   the JPEG ladder (rungs at RATE)          one .mov per clip, HAP, frame k = rung k
//!   frames/<clip>/%06d.jpg  ──ffmpeg -c:v hap──►  hap/<clip>.mov + hap/<clip>.json
//!                                                   (packet offsets, read by ffprobe)
//!                                │
//!            Hap::frame(k): seek, read one packet, unwrap the HAP header,
//!            snappy-decompress (per chunk) → BC1 / BC3 blocks
//!                                │
//!       GPU with BC textures ────┴──── GPU without (most phones, llvmpipe)
//!       upload the blocks as is         `Frame::rgba` decodes them on the CPU
//! ```
//!
//! A JPEG rung costs a full decode plus an RGBA upload four times its
//! compressed size; a HAP rung costs a decompression that runs at memory speed
//! and an upload of **a sixth (BC1) or a quarter (BC3)** of the RGBA. That is
//! the reason to bother: the ladder stays the address book (the descriptors are
//! read from it, see `index`), HAP is how a rung reaches the screen.
//!
//! The proxy is encoded **from the ladder itself**, so frame `k` of the `.mov`
//! is rung `k` by construction — no second seek grid to keep in step.
//!
//! The snappy decoder is here rather than a crate: it is sixty lines, the
//! format is frozen, and this crate's rule is to build on a host with nothing.

use metal_vfs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What the blocks of one frame are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Texture {
    /// `hap`: BC1 (DXT1), RGB, 8 bytes per 4×4 block.
    Bc1,
    /// `hap_alpha`: BC3 (DXT5), RGBA, 16 bytes per block.
    Bc3,
    /// `hap_q`: BC3 holding scaled YCoCg. No GPU format samples it as colour,
    /// so it always takes the CPU path.
    YCoCg,
}

impl Texture {
    fn from_nibble(n: u8) -> Option<Texture> {
        match n {
            0xB => Some(Texture::Bc1),
            0xE => Some(Texture::Bc3),
            0xF => Some(Texture::YCoCg),
            _ => None,
        }
    }

    /// Bytes per 4×4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            Texture::Bc1 => 8,
            Texture::Bc3 | Texture::YCoCg => 16,
        }
    }
}

/// One decoded HAP frame: its blocks, still compressed for the GPU.
#[derive(Clone, Debug)]
pub struct Frame {
    pub texture: Texture,
    pub width: u32,
    pub height: u32,
    pub blocks: Vec<u8>,
}

impl Frame {
    /// The frame as RGBA8 — for a GPU that cannot sample BC, and for tests.
    pub fn rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let bw = w.div_ceil(4);
        let mut out = vec![0u8; w * h * 4];
        let size = self.texture.block_bytes();
        for (b, block) in self.blocks.chunks_exact(size).enumerate() {
            let (bx, by) = (b % bw, b / bw);
            if by * 4 >= h {
                break;
            }
            let px = match self.texture {
                Texture::Bc1 => bc1_block(block, true),
                Texture::Bc3 => bc3_block(block),
                Texture::YCoCg => ycocg(bc3_block(block)),
            };
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    out[(y * w + x) * 4..][..4].copy_from_slice(p);
                }
            }
        }
        out
    }
}

/// The sidecar `ffprobe` writes once at ingest: where each packet is.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Index {
    pub width: u32,
    pub height: u32,
    /// `(byte offset, byte length)` per frame, in presentation order.
    pub packets: Vec<(u64, u32)>,
    /// The proxy spec this movie was encoded to (`index::PROXY_SPEC`). Absent
    /// in a proxy made before the spec was one thing: that is `0`, and it is
    /// rebuilt rather than read.
    #[serde(default)]
    pub spec: u32,
}

/// One clip's HAP proxy, open for reading.
pub struct Hap {
    file: File,
    pub index: Index,
}

impl Hap {
    /// `hap/<clip>.mov` beside `hap/<clip>.json`.
    pub fn paths(root: &Path, clip: &str) -> (PathBuf, PathBuf) {
        let dir = root.join("hap");
        (dir.join(format!("{clip}.mov")), dir.join(format!("{clip}.json")))
    }

    pub fn open(movie: &Path, index: &Path) -> Result<Hap, String> {
        let text = metal_vfs::read_to_string(index).map_err(|e| format!("{}: {e}", index.display()))?;
        let index: Index = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", index.display()))?;
        let file = File::open(movie).map_err(|e| format!("{}: {e}", movie.display()))?;
        Ok(Hap { file, index })
    }

    pub fn frames(&self) -> usize {
        self.index.packets.len()
    }

    /// Frame `k`, clamped to the last.
    pub fn frame(&mut self, k: usize) -> Result<Frame, String> {
        let &(pos, len) = self
            .index
            .packets
            .get(k.min(self.index.packets.len().saturating_sub(1)))
            .ok_or("no frames")?;
        let mut packet = vec![0u8; len as usize];
        self.file.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        self.file.read_exact(&mut packet).map_err(|e| e.to_string())?;
        decode(&packet, self.index.width, self.index.height)
    }
}

/// A section header: `(payload length, type byte, header length)`.
fn section(b: &[u8]) -> Result<(usize, u8, usize), String> {
    if b.len() < 4 {
        return Err("short section".into());
    }
    let len = u32::from_le_bytes([b[0], b[1], b[2], 0]) as usize;
    if len == 0 {
        if b.len() < 8 {
            return Err("short section".into());
        }
        Ok((u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize, b[3], 8))
    } else {
        Ok((len, b[3], 4))
    }
}

/// One HAP packet → its texture blocks.
pub fn decode(packet: &[u8], width: u32, height: u32) -> Result<Frame, String> {
    let (len, kind, head) = section(packet)?;
    let body = packet.get(head..head + len).ok_or("truncated frame")?;
    let texture = Texture::from_nibble(kind & 0x0F).ok_or_else(|| format!("unsupported HAP texture {kind:#x}"))?;
    let want = (width.div_ceil(4) * height.div_ceil(4)) as usize * texture.block_bytes();
    let blocks = match kind >> 4 {
        0xA => body.to_vec(),
        0xB => snappy(body)?,
        0xC => chunked(body)?,
        c => return Err(format!("unsupported HAP compressor {c:#x}")),
    };
    if blocks.len() < want {
        return Err(format!("frame has {} bytes, {width}×{height} needs {want}", blocks.len()));
    }
    Ok(Frame { texture, width, height, blocks })
}

/// A chunked frame: a decode-instructions container, then the chunks.
fn chunked(body: &[u8]) -> Result<Vec<u8>, String> {
    let (len, kind, head) = section(body)?;
    if kind != 0x01 {
        return Err("chunked frame without decode instructions".into());
    }
    let instructions = body.get(head..head + len).ok_or("truncated instructions")?;
    let data = &body[head + len..];
    let (mut compressors, mut sizes, mut offsets) = (Vec::new(), Vec::new(), None);
    let mut at = 0;
    while at < instructions.len() {
        let (l, k, h) = section(&instructions[at..])?;
        let v = instructions.get(at + h..at + h + l).ok_or("truncated instruction")?;
        match k {
            0x02 => compressors = v.to_vec(),
            0x03 => sizes = v.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize).collect(),
            0x04 => offsets = Some(v.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize).collect::<Vec<_>>()),
            _ => {}
        }
        at += h + l;
    }
    let mut out = Vec::new();
    let mut next = 0;
    for (i, &size) in sizes.iter().enumerate() {
        let start = offsets.as_ref().map_or(next, |o| o[i]);
        let chunk = data.get(start..start + size).ok_or("truncated chunk")?;
        match compressors.get(i) {
            Some(0x0A) => out.extend_from_slice(chunk),
            Some(0x0B) => out.extend(snappy(chunk)?),
            c => return Err(format!("unsupported chunk compressor {c:?}")),
        }
        next = start + size;
    }
    Ok(out)
}

/// Raw snappy (no framing), as HAP stores it.
pub fn snappy(src: &[u8]) -> Result<Vec<u8>, String> {
    let (mut n, mut shift, mut at) = (0usize, 0, 0);
    loop {
        let b = *src.get(at).ok_or("snappy: no length")?;
        n |= ((b & 0x7F) as usize) << shift;
        at += 1;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return Err("snappy: bad length".into());
        }
    }
    let mut out = Vec::with_capacity(n);
    let bad = || "snappy: truncated".to_string();
    while at < src.len() {
        let tag = src[at];
        at += 1;
        let (len, offset) = match tag & 3 {
            0 => {
                let mut len = (tag >> 2) as usize;
                if len >= 60 {
                    let extra = len - 59;
                    let b = src.get(at..at + extra).ok_or_else(bad)?;
                    len = b.iter().rev().fold(0, |a, &x| (a << 8) | x as usize);
                    at += extra;
                }
                let lit = src.get(at..at + len + 1).ok_or_else(bad)?;
                out.extend_from_slice(lit);
                at += len + 1;
                continue;
            }
            1 => {
                let b = *src.get(at).ok_or_else(bad)? as usize;
                at += 1;
                (4 + ((tag >> 2) & 7) as usize, ((tag as usize >> 5) << 8) | b)
            }
            2 => {
                let b = src.get(at..at + 2).ok_or_else(bad)?;
                at += 2;
                ((tag >> 2) as usize + 1, u16::from_le_bytes([b[0], b[1]]) as usize)
            }
            _ => {
                let b = src.get(at..at + 4).ok_or_else(bad)?;
                at += 4;
                ((tag >> 2) as usize + 1, u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            }
        };
        if offset == 0 || offset > out.len() {
            return Err("snappy: bad offset".into());
        }
        let from = out.len() - offset;
        for i in 0..len {
            let byte = out[from + i];
            out.push(byte);
        }
    }
    if out.len() != n {
        return Err(format!("snappy: {} bytes, header said {n}", out.len()));
    }
    Ok(out)
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [((r * 527 + 23) >> 6) as u8, ((g * 259 + 33) >> 6) as u8, ((b * 527 + 23) >> 6) as u8]
}

fn mix(a: [u8; 3], b: [u8; 3], wa: u32, wb: u32) -> [u8; 4] {
    let t = wa + wb;
    let m = |i: usize| ((a[i] as u32 * wa + b[i] as u32 * wb) / t) as u8;
    [m(0), m(1), m(2), 255]
}

/// Sixteen pixels of one BC1 block, row-major. `four` forces the four-colour
/// mode, which is how BC3's colour half is always read.
fn bc1_block(b: &[u8], punch: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let palette = if c0 > c1 || !punch {
        [[p0[0], p0[1], p0[2], 255], [p1[0], p1[1], p1[2], 255], mix(p0, p1, 2, 1), mix(p0, p1, 1, 2)]
    } else {
        [[p0[0], p0[1], p0[2], 255], [p1[0], p1[1], p1[2], 255], mix(p0, p1, 1, 1), [0, 0, 0, 0]]
    };
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    std::array::from_fn(|i| palette[((bits >> (2 * i)) & 3) as usize])
}

fn bc3_block(b: &[u8]) -> [[u8; 4]; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let alphas: [u8; 8] = std::array::from_fn(|i| match i {
        0 => a0 as u8,
        1 => a1 as u8,
        _ if a0 > a1 => ((a0 * (8 - i as u32) + a1 * (i as u32 - 1)) / 7) as u8,
        6 => 0,
        7 => 255,
        _ => ((a0 * (6 - i as u32) + a1 * (i as u32 - 1)) / 5) as u8,
    });
    let bits = b[2..8].iter().rev().fold(0u64, |a, &x| (a << 8) | x as u64);
    let mut px = bc1_block(&b[8..16], false);
    for (i, p) in px.iter_mut().enumerate() {
        p[3] = alphas[((bits >> (3 * i)) & 7) as usize];
    }
    px
}

/// HAP Q's scaled YCoCg (Co in R, Cg in G, scale in B, Y in A) → RGB.
fn ycocg(px: [[u8; 4]; 16]) -> [[u8; 4]; 16] {
    px.map(|[co, cg, s, y]| {
        let scale = 1.0 / ((s as f32 / 255.0) * (255.0 / 8.0) + 1.0);
        let co = (co as f32 / 255.0 - 0.5 * 256.0 / 255.0) * scale;
        let cg = (cg as f32 / 255.0 - 0.5 * 256.0 / 255.0) * scale;
        let y = y as f32 / 255.0;
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [q(y + co - cg), q(y + cg), q(y - co - cg), 255]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snappy_reads_literals_and_overlapping_copies() {
        // 10 bytes: literal "ab", then a 1-byte-offset copy of 8 at offset 2.
        let src = [10, 0b0000_0100, b'a', b'b', 0b0001_0001, 2];
        assert_eq!(snappy(&src).unwrap(), b"ababababab");
        assert!(snappy(&[3, 0, b'a']).is_err());
    }

    #[test]
    fn an_uncompressed_bc1_frame_decodes_to_its_colours() {
        // One 4×4 block: c0 = pure red, c1 = pure blue, every index 0 → red.
        let block = [0x00, 0xF8, 0x1F, 0x00, 0, 0, 0, 0];
        let mut packet = vec![8, 0, 0, 0xAB];
        packet.extend_from_slice(&block);
        let f = decode(&packet, 4, 4).unwrap();
        assert_eq!(f.texture, Texture::Bc1);
        assert_eq!(&f.rgba()[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn a_chunked_frame_joins_its_chunks() {
        let a = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let b = [0x1F, 0x00, 0x1F, 0x00, 0, 0, 0, 0];
        let mut ins = vec![2, 0, 0, 0x02, 0x0A, 0x0A];
        ins.extend([8, 0, 0, 0x03]);
        ins.extend(8u32.to_le_bytes());
        ins.extend(8u32.to_le_bytes());
        let mut body = vec![ins.len() as u8, 0, 0, 0x01];
        body.extend(ins);
        body.extend(a);
        body.extend(b);
        let mut packet = vec![body.len() as u8, 0, 0, 0xCB];
        packet.extend(body);
        let f = decode(&packet, 8, 4).unwrap();
        let px = f.rgba();
        assert_eq!(&px[..4], &[255, 0, 0, 255]);
        assert_eq!(&px[4 * 4..4 * 4 + 4], &[0, 0, 255, 255]);
    }

    /// The whole path on a real ffmpeg: a ladder → HAP → frame `k` looks like
    /// rung `k`. Skipped where ffmpeg has no `hap` encoder.
    #[test]
    fn a_ladder_encodes_to_hap_and_reads_back_rung_for_rung() {
        let dir = std::env::temp_dir().join(format!("fw-hap-{}", std::process::id()));
        let ladder = dir.join("frames");
        metal_vfs::create_dir_all(&ladder).unwrap();
        let made = metal_schema::platform::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=130x74:rate=12", "-frames:v", "6", "-q:v", "2"])
            .arg(ladder.join("%06d.jpg"))
            .status();
        if !made.is_ok_and(|s| s.success()) {
            return;
        }
        let movie = dir.join("hap").join("c.mov");
        let Ok((w, h, packets)) = metal_ingest::video::encode_hap(&ladder, 12.0, &movie, false) else {
            return;
        };
        assert_eq!((w, h, packets.len()), (128, 72, 6));
        let index = dir.join("hap").join("c.json");
        metal_vfs::write(&index, serde_json::to_string(&Index { width: w, height: h, packets, spec: crate::index::PROXY_SPEC }).unwrap()).unwrap();
        let mut hap = Hap::open(&movie, &index).unwrap();
        for k in [0, 5] {
            let f = hap.frame(k).unwrap();
            assert_eq!(f.texture, Texture::Bc1);
            let rgba = f.rgba();
            let jpeg = crate::index::decode(&ladder.join(format!("{:06}.jpg", k + 1))).unwrap();
            // Mean absolute error over the shared area — BC1 is lossy, a wrong
            // frame or a wrong decode is off by far more than this.
            let mut err = 0u64;
            for y in 0..h as usize {
                for x in 0..w as usize {
                    for c in 0..3 {
                        let a = rgba[(y * w as usize + x) * 4 + c] as i64;
                        let b = jpeg.px[(y * jpeg.width as usize + x) * 3 + c] as i64;
                        err += (a - b).unsigned_abs();
                    }
                }
            }
            let mean = err as f64 / (w * h * 3) as f64;
            assert!(mean < 12.0, "frame {k}: mean error {mean}");
        }
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The whole point of the direct-from-source encoder: a source that really
    /// carries alpha comes back as BC3 with transparency still in it. The JPEG
    /// ladder this replaced has no alpha, so a DXV alpha export went through it
    /// opaque — hence the test starts from a source file, not a ladder.
    ///
    /// `testsrc` is opaque, so `geq` writes a horizontal alpha ramp into an
    /// `rgba` frame; `qtrle` then stores it losslessly. If the build cannot do
    /// that (`qtrle`/`geq` absent, no `hap_alpha` encoder) the test returns
    /// rather than fails, matching the ladder test above.
    #[test]
    fn an_alpha_source_encodes_to_bc3_with_real_alpha() {
        if !metal_schema::platform::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("fw-hap-alpha-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();

        let src = dir.join("src.mov");
        let made = metal_schema::platform::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=64x64:rate=8:duration=1"])
            .args(["-vf", "format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='X*4'"])
            .args(["-c:v", "qtrle", "-pix_fmt", "rgba"])
            .arg(&src)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            let _ = metal_vfs::remove_dir_all(&dir);
            return;
        }
        // The source must actually state an alpha format, or there is nothing
        // to carry and the test would be checking the wrong thing.
        let info = metal_ingest::video::probe(&src).unwrap();
        assert!(metal_ingest::video::has_alpha(&info.pix_fmt), "source pix_fmt {}", info.pix_fmt);

        let movie = dir.join("hap").join("c.mov");
        let Ok((w, h, packets)) =
            metal_ingest::video::encode_hap_video(&src, 8.0, 64, 8, &movie, "", true, &mut |_| {})
        else {
            let _ = metal_vfs::remove_dir_all(&dir);
            return;
        };
        assert_eq!(packets.len(), 8);
        let index = dir.join("hap").join("c.json");
        metal_vfs::write(
            &index,
            serde_json::to_string(&Index { width: w, height: h, packets, spec: crate::index::PROXY_SPEC }).unwrap(),
        )
        .unwrap();

        let mut hap = Hap::open(&movie, &index).unwrap();
        let f = hap.frame(0).unwrap();
        assert_eq!(f.texture, Texture::Bc3, "alpha must select BC3/DXT5");
        // The ramp is real, so the round trip must preserve some transparency
        // rather than flatten it to 255. Every fourth byte is the alpha lane —
        // the ramp's own 0 byte proves the channel survived.
        let rgba = f.rgba();
        let min = rgba.chunks_exact(4).map(|p| p[3]).min().unwrap_or(255);
        assert!(min < 255, "every alpha byte came back opaque (min {min})");
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
