//! Linux virtual camera: frames written into a v4l2loopback device, which
//! every video-call program then lists as a webcam.
//!
//! The module is loaded once, by hand (root):
//!
//! ```sh
//! sudo modprobe v4l2loopback devices=1 video_nr=10 card_label="MetalVJ" exclusive_caps=1
//! ```
//!
//! `exclusive_caps=1` is what makes Chrome/Zoom/OBS see it as a capture
//! device once a writer has set the format. The sink sets the format itself
//! (`VIDIOC_S_FMT` on the output queue) and then `write()`s whole frames.

use crate::{convert, Sink};

/// Where the kernel lists a loaded module.
const MODULE_DIR: &str = "/sys/module/v4l2loopback";
const CLASS_DIR: &str = "/sys/devices/virtual/video4linux";

/// The line the disabled reason and the docs both quote.
pub const MODPROBE: &str =
    "sudo modprobe v4l2loopback devices=1 video_nr=10 card_label=\"MetalVJ\" exclusive_caps=1";

/// One loopback device: `/dev/videoN` and its card label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loopback {
    pub path: String,
    pub label: String,
}

/// Why the mode is off, or `None` when at least one loopback device exists.
pub fn disabled_reason() -> Option<String> {
    disabled_reason_in(std::path::Path::new(MODULE_DIR), std::path::Path::new(CLASS_DIR))
}

pub(crate) fn disabled_reason_in(module: &std::path::Path, class: &std::path::Path) -> Option<String> {
    if !cfg!(target_os = "linux") {
        return Some("the virtual camera on this system needs a signed driver (pending)".into());
    }
    if !module.exists() {
        return Some(format!("the v4l2loopback module is not loaded: {MODPROBE}"));
    }
    if devices_in(class).is_empty() {
        return Some(format!("v4l2loopback is loaded but created no devices: {MODPROBE}"));
    }
    None
}

/// Every loopback device the module created.
pub fn devices() -> Vec<Loopback> {
    devices_in(std::path::Path::new(CLASS_DIR))
}

pub(crate) fn devices_in(class: &std::path::Path) -> Vec<Loopback> {
    let Ok(rd) = std::fs::read_dir(class) else { return Vec::new() };
    let mut out: Vec<Loopback> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_prefix("video")?.parse::<u32>().ok()?;
            // A loopback device's sysfs node is virtual and carries its own
            // `name`; a real webcam is not under `devices/virtual`.
            let label = std::fs::read_to_string(e.path().join("name")).ok()?.trim().to_string();
            Some(Loopback { path: format!("/dev/{name}"), label })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Pixel format the sink writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Yuyv,
    Rgb24,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        match s {
            "" | "yuyv" => Some(Format::Yuyv),
            "rgb24" => Some(Format::Rgb24),
            _ => None,
        }
    }
    fn fourcc(self) -> u32 {
        let f = |s: &[u8; 4]| u32::from_le_bytes(*s);
        match self {
            Format::Yuyv => f(b"YUYV"),
            Format::Rgb24 => f(b"RGB3"),
        }
    }
    fn bytes_per_line(self, w: u32) -> u32 {
        match self {
            Format::Yuyv => w.div_ceil(2) * 4,
            Format::Rgb24 => w * 3,
        }
    }
    pub fn pack(self, rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
        match self {
            Format::Yuyv => convert::rgba_to_yuyv(rgba, w, h),
            Format::Rgb24 => convert::rgba_to_rgb24(rgba),
        }
    }
}

/// `target` is `/dev/videoN` or `/dev/videoN:rgb24`.
pub fn parse_target(target: &str) -> Result<(String, Format), String> {
    let (dev, fmt) = target.split_once(':').unwrap_or((target, ""));
    let n = dev.strip_prefix("/dev/video").ok_or_else(|| format!("not a video device: {dev}"))?;
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("not a video device: {dev}"));
    }
    let fmt = Format::parse(fmt).ok_or_else(|| format!("unknown format: {fmt}"))?;
    Ok((dev.to_string(), fmt))
}

/// The open device. The format is (re)set whenever the frame size changes.
pub struct VirtualCam {
    path: String,
    format: Format,
    #[cfg(target_os = "linux")]
    file: Option<std::fs::File>,
    size: (u32, u32),
}

impl VirtualCam {
    pub fn open(target: &str) -> Result<VirtualCam, String> {
        if let Some(why) = disabled_reason() {
            return Err(why);
        }
        let (path, format) = parse_target(target)?;
        Ok(VirtualCam {
            path,
            format,
            #[cfg(target_os = "linux")]
            file: None,
            size: (0, 0),
        })
    }
}

#[cfg(target_os = "linux")]
mod ioctl {
    /// `struct v4l2_pix_format`.
    #[repr(C)]
    #[derive(Default)]
    pub struct PixFormat {
        pub width: u32,
        pub height: u32,
        pub pixelformat: u32,
        pub field: u32,
        pub bytesperline: u32,
        pub sizeimage: u32,
        pub colorspace: u32,
        pub priv_: u32,
        pub flags: u32,
        pub ycbcr_enc: u32,
        pub quantization: u32,
        pub xfer_func: u32,
    }
    /// `struct v4l2_format`: `type` then a 200-byte union, 8-aligned.
    #[repr(C, align(8))]
    pub struct Format {
        pub type_: u32,
        pub _pad: u32,
        pub pix: PixFormat,
        pub _rest: [u8; 200 - std::mem::size_of::<PixFormat>()],
    }
    pub const BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
    pub const FIELD_NONE: u32 = 1;
    pub const COLORSPACE_SRGB: u32 = 8;
    /// `_IOWR('V', 5, struct v4l2_format)`.
    pub const VIDIOC_S_FMT: u64 =
        (3u64 << 30) | ((std::mem::size_of::<Format>() as u64) << 16) | ((b'V' as u64) << 8) | 5;
}

impl Sink for VirtualCam {
    fn push(&mut self, rgba: &[u8], w: u32, h: u32, _t: f64) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use std::io::Write;
            use std::os::fd::AsRawFd;
            if self.file.is_none() || self.size != (w, h) {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&self.path)
                    .map_err(|e| format!("{}: {e}", self.path))?;
                let bpl = self.format.bytes_per_line(w);
                let mut f = ioctl::Format {
                    type_: ioctl::BUF_TYPE_VIDEO_OUTPUT,
                    _pad: 0,
                    pix: ioctl::PixFormat {
                        width: w,
                        height: h,
                        pixelformat: self.format.fourcc(),
                        field: ioctl::FIELD_NONE,
                        bytesperline: bpl,
                        sizeimage: bpl * h,
                        colorspace: ioctl::COLORSPACE_SRGB,
                        ..Default::default()
                    },
                    _rest: [0; 200 - std::mem::size_of::<ioctl::PixFormat>()],
                };
                // SAFETY: a valid fd and a correctly sized, initialised struct.
                let r = unsafe { libc::ioctl(file.as_raw_fd(), ioctl::VIDIOC_S_FMT as _, &mut f) };
                if r < 0 {
                    return Err(format!("VIDIOC_S_FMT en {}: {}", self.path, std::io::Error::last_os_error()));
                }
                self.file = Some(file);
                self.size = (w, h);
            }
            let bytes = self.format.pack(rgba, w, h);
            let file = self.file.as_mut().expect("opened above");
            if let Err(e) = file.write_all(&bytes) {
                self.file = None;
                return Err(format!("{}: {e}", self.path));
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (rgba, w, h, &self.path, self.format, self.size);
            Err("the virtual camera only exists on Linux".into())
        }
    }

    fn describe(&self) -> String {
        format!("virtualcam {}", self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_parse_device_and_format() {
        assert_eq!(parse_target("/dev/video10").unwrap(), ("/dev/video10".into(), Format::Yuyv));
        assert_eq!(parse_target("/dev/video3:rgb24").unwrap(), ("/dev/video3".into(), Format::Rgb24));
        assert!(parse_target("/etc/passwd").is_err());
        assert!(parse_target("/dev/video1;rm").is_err());
        assert!(parse_target("/dev/video1:nv12").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_ioctl_struct_matches_the_kernel_size() {
        assert_eq!(std::mem::size_of::<ioctl::Format>(), 208);
        assert_eq!(ioctl::VIDIOC_S_FMT, 0xC0D0_5605);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_module_reads_as_the_modprobe_line() {
        let dir = std::env::temp_dir().join(format!("mvj-v4l2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let why = disabled_reason_in(&dir.join("module"), &dir.join("class")).unwrap();
        assert!(why.contains("modprobe v4l2loopback"), "{why}");
        std::fs::create_dir_all(dir.join("module")).unwrap();
        let why = disabled_reason_in(&dir.join("module"), &dir.join("class")).unwrap();
        assert!(why.contains("created no devices"), "{why}");
        std::fs::create_dir_all(dir.join("class/video10")).unwrap();
        std::fs::write(dir.join("class/video10/name"), "MetalVJ\n").unwrap();
        assert_eq!(disabled_reason_in(&dir.join("module"), &dir.join("class")), None);
        assert_eq!(
            devices_in(&dir.join("class")),
            vec![Loopback { path: "/dev/video10".into(), label: "MetalVJ".into() }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Needs `modprobe v4l2loopback` (see [`MODPROBE`]); writes one grey
    /// frame into the first loopback device.
    #[test]
    #[ignore]
    fn hardware_writes_a_frame_into_the_loopback() {
        let dev = devices().into_iter().next().expect("load v4l2loopback first");
        let mut cam = VirtualCam::open(&dev.path).unwrap();
        let px = vec![128u8; 640 * 360 * 4];
        cam.push(&px, 640, 360, 0.0).unwrap();
    }
}
