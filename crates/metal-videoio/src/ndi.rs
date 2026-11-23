//! NDI send and receive over the NDI runtime, opened at run time with
//! `libloading` — never linked at build time, so a machine without the
//! runtime builds and runs the same and only lists the mode as disabled.
//!
//! Where the library is looked for, in order: `$NDI_RUNTIME_DIR_V6`,
//! `$NDI_RUNTIME_DIR_V5` (the installers set these), then the loader's own
//! search for the platform's names (`libndi.so.6`, `libndi.so.5`,
//! `libndi.so`; `Processing.NDI.Lib.x64.dll`; `libndi.dylib`).

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::{Sink, Source};

#[cfg(target_os = "linux")]
const NAMES: &[&str] = &["libndi.so.6", "libndi.so.5", "libndi.so"];
#[cfg(target_os = "windows")]
const NAMES: &[&str] = &["Processing.NDI.Lib.x64.dll"];
#[cfg(target_os = "macos")]
const NAMES: &[&str] = &["libndi.dylib", "libndi_advanced.dylib"];
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const NAMES: &[&str] = &[];

const FOURCC_RGBA: u32 = u32::from_le_bytes(*b"RGBA");
const FRAME_FORMAT_PROGRESSIVE: i32 = 1;
const RECV_COLOR_RGBX_RGBA: i32 = 2;
const RECV_BANDWIDTH_HIGHEST: i32 = 100;
const FRAME_TYPE_VIDEO: i32 = 1;

#[repr(C)]
struct SendCreate {
    p_ndi_name: *const c_char,
    p_groups: *const c_char,
    clock_video: bool,
    clock_audio: bool,
}

#[repr(C)]
struct VideoFrameV2 {
    xres: i32,
    yres: i32,
    fourcc: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    picture_aspect_ratio: f32,
    frame_format_type: i32,
    timecode: i64,
    p_data: *mut u8,
    line_stride_in_bytes: i32,
    p_metadata: *const c_char,
    timestamp: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SourceT {
    p_ndi_name: *const c_char,
    p_url_address: *const c_char,
}

#[repr(C)]
struct FindCreate {
    show_local_sources: bool,
    p_groups: *const c_char,
    p_extra_ips: *const c_char,
}

#[repr(C)]
struct RecvCreateV3 {
    source_to_connect_to: SourceT,
    color_format: i32,
    bandwidth: i32,
    allow_video_fields: bool,
    p_ndi_recv_name: *const c_char,
}

type Handle = *mut c_void;

/// The resolved entry points.
struct Api {
    _lib: libloading::Library,
    send_create: unsafe extern "C" fn(*const SendCreate) -> Handle,
    send_destroy: unsafe extern "C" fn(Handle),
    send_video_v2: unsafe extern "C" fn(Handle, *const VideoFrameV2),
    find_create_v2: unsafe extern "C" fn(*const FindCreate) -> Handle,
    find_destroy: unsafe extern "C" fn(Handle),
    find_wait_for_sources: unsafe extern "C" fn(Handle, u32) -> bool,
    find_get_current_sources: unsafe extern "C" fn(Handle, *mut u32) -> *const SourceT,
    recv_create_v3: unsafe extern "C" fn(*const RecvCreateV3) -> Handle,
    recv_destroy: unsafe extern "C" fn(Handle),
    recv_capture_v2: unsafe extern "C" fn(Handle, *mut VideoFrameV2, *mut c_void, *mut c_void, u32) -> i32,
    recv_free_video_v2: unsafe extern "C" fn(Handle, *const VideoFrameV2),
}

// SAFETY: the NDI SDK documents its handles as usable from any thread.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

fn candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for var in ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"] {
        if let Some(dir) = std::env::var_os(var) {
            for n in NAMES {
                out.push(std::path::Path::new(&dir).join(n));
            }
        }
    }
    out.extend(NAMES.iter().map(std::path::PathBuf::from));
    out
}

fn load() -> Result<Api, String> {
    let mut last = String::from("no NDI library name for this system");
    for path in candidates() {
        // SAFETY: loading a shared library runs its initialisers; the NDI
        // runtime is the vendor's own library, opened by the name it installs.
        let lib = match unsafe { libloading::Library::new(&path) } {
            Ok(l) => l,
            Err(e) => {
                last = e.to_string();
                continue;
            }
        };
        macro_rules! sym {
            ($name:literal) => {
                // SAFETY: the signature matches `Processing.NDI.*.h`.
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| format!("{}: {}", $name, e))?
            };
        }
        let initialize: unsafe extern "C" fn() -> bool = sym!("NDIlib_initialize");
        // SAFETY: no arguments; false means the CPU is unsupported.
        if !unsafe { initialize() } {
            return Err("NDI runtime failed to start (CPU not supported)".into());
        }
        let api = Api {
            send_create: sym!("NDIlib_send_create"),
            send_destroy: sym!("NDIlib_send_destroy"),
            send_video_v2: sym!("NDIlib_send_send_video_v2"),
            find_create_v2: sym!("NDIlib_find_create_v2"),
            find_destroy: sym!("NDIlib_find_destroy"),
            find_wait_for_sources: sym!("NDIlib_find_wait_for_sources"),
            find_get_current_sources: sym!("NDIlib_find_get_current_sources"),
            recv_create_v3: sym!("NDIlib_recv_create_v3"),
            recv_destroy: sym!("NDIlib_recv_destroy"),
            recv_capture_v2: sym!("NDIlib_recv_capture_v2"),
            recv_free_video_v2: sym!("NDIlib_recv_free_video_v2"),
            _lib: lib,
        };
        return Ok(api);
    }
    Err(format!("NDI runtime not installed ({last}); install NDI Tools/Runtime from ndi.video"))
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

/// Why NDI is off, or `None` when the runtime loaded.
pub fn disabled_reason() -> Option<String> {
    api().err()
}

/// A named NDI sender.
pub struct NdiSend {
    api: &'static Api,
    handle: Handle,
    name: String,
}

// SAFETY: see `Api`.
unsafe impl Send for NdiSend {}

impl NdiSend {
    pub fn open(name: &str) -> Result<NdiSend, String> {
        let api = api()?;
        let name = if name.trim().is_empty() { "MetalVJ" } else { name.trim() };
        let cname = CString::new(name).map_err(|e| e.to_string())?;
        let create = SendCreate {
            p_ndi_name: cname.as_ptr(),
            p_groups: std::ptr::null(),
            clock_video: false,
            clock_audio: false,
        };
        // SAFETY: a valid descriptor; the name is copied by the SDK.
        let handle = unsafe { (api.send_create)(&create) };
        if handle.is_null() {
            return Err(format!("NDI could not create sender {name:?}"));
        }
        Ok(NdiSend { api, handle, name: name.to_string() })
    }
}

impl Sink for NdiSend {
    fn push(&mut self, rgba: &[u8], w: u32, h: u32, t: f64) -> Result<(), String> {
        if rgba.len() < (w * h * 4) as usize {
            return Err("frame shorter than w×h×4".into());
        }
        let frame = VideoFrameV2 {
            xres: w as i32,
            yres: h as i32,
            fourcc: FOURCC_RGBA,
            frame_rate_n: 30000,
            frame_rate_d: 1000,
            picture_aspect_ratio: 0.0,
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            timecode: (t * 1e7) as i64,
            p_data: rgba.as_ptr() as *mut u8,
            line_stride_in_bytes: (w * 4) as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY: the synchronous send copies before returning.
        unsafe { (self.api.send_video_v2)(self.handle, &frame) };
        Ok(())
    }
    fn describe(&self) -> String {
        format!("ndi {}", self.name)
    }
}

impl Drop for NdiSend {
    fn drop(&mut self) {
        // SAFETY: created by `send_create`, destroyed once.
        unsafe { (self.api.send_destroy)(self.handle) };
    }
}

/// The NDI sources visible on the network, waiting up to `wait` for them.
pub fn sources(wait: Duration) -> Vec<String> {
    let Ok(api) = api() else { return Vec::new() };
    let fc = FindCreate { show_local_sources: true, p_groups: std::ptr::null(), p_extra_ips: std::ptr::null() };
    // SAFETY: valid descriptor; every pointer read below lives until destroy.
    unsafe {
        let find = (api.find_create_v2)(&fc);
        if find.is_null() {
            return Vec::new();
        }
        (api.find_wait_for_sources)(find, wait.as_millis() as u32);
        let mut n = 0u32;
        let list = (api.find_get_current_sources)(find, &mut n);
        let mut out = Vec::new();
        for i in 0..n as usize {
            let s = *list.add(i);
            if !s.p_ndi_name.is_null() {
                out.push(CStr::from_ptr(s.p_ndi_name).to_string_lossy().to_string());
            }
        }
        (api.find_destroy)(find);
        out
    }
}

/// A receiver on one source, its newest frame kept by a thread.
pub struct NdiReceive {
    latest: Arc<Mutex<Option<(Vec<u8>, u32, u32)>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl NdiReceive {
    pub fn open(source: &str) -> Result<NdiReceive, String> {
        let api = api()?;
        let cname = CString::new(source).map_err(|e| e.to_string())?;
        let rc = RecvCreateV3 {
            source_to_connect_to: SourceT { p_ndi_name: cname.as_ptr(), p_url_address: std::ptr::null() },
            color_format: RECV_COLOR_RGBX_RGBA,
            bandwidth: RECV_BANDWIDTH_HIGHEST,
            allow_video_fields: false,
            p_ndi_recv_name: std::ptr::null(),
        };
        // SAFETY: valid descriptor; the SDK copies the strings.
        let handle = unsafe { (api.recv_create_v3)(&rc) };
        if handle.is_null() {
            return Err(format!("NDI could not connect to {source:?}"));
        }
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (l, s) = (Arc::clone(&latest), Arc::clone(&stop));
        let h = handle as usize;
        std::thread::spawn(move || {
            let handle = h as Handle;
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                // SAFETY: zeroed is a valid empty frame; freed right after copy.
                unsafe {
                    let mut f: VideoFrameV2 = std::mem::zeroed();
                    let kind = (api.recv_capture_v2)(handle, &mut f, std::ptr::null_mut(), std::ptr::null_mut(), 200);
                    if kind == FRAME_TYPE_VIDEO && !f.p_data.is_null() && f.xres > 0 && f.yres > 0 {
                        let (w, h) = (f.xres as u32, f.yres as u32);
                        let stride = f.line_stride_in_bytes.max(f.xres * 4) as usize;
                        let mut px = Vec::with_capacity((w * h * 4) as usize);
                        for y in 0..h as usize {
                            px.extend_from_slice(std::slice::from_raw_parts(f.p_data.add(y * stride), w as usize * 4));
                        }
                        (api.recv_free_video_v2)(handle, &f);
                        if let Ok(mut g) = l.lock() {
                            *g = Some((px, w, h));
                        }
                    }
                }
            }
            // SAFETY: created above, destroyed once by its only owner.
            unsafe { (api.recv_destroy)(handle) };
        });
        Ok(NdiReceive { latest, stop })
    }
}

impl Source for NdiReceive {
    fn frame(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        self.latest.lock().ok()?.clone()
    }
}

impl Drop for NdiReceive {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The `capture` node's `ndi:<name>` source: one shared receiver per name,
/// dropped when nobody asked for it in a few seconds (same rule as the ffmpeg
/// captures — a connection nobody looks at is a bug).
pub fn receive_frame(source: &str) -> Option<(Vec<u8>, u32, u32)> {
    struct Held {
        rx: NdiReceive,
        asked: Instant,
    }
    static REG: OnceLock<Mutex<HashMap<String, Held>>> = OnceLock::new();
    let mut reg = REG.get_or_init(Default::default).lock().ok()?;
    let now = Instant::now();
    reg.retain(|k, h| k == source || now.duration_since(h.asked) < Duration::from_secs(5));
    if !reg.contains_key(source) {
        let rx = NdiReceive::open(source).ok()?;
        reg.insert(source.to_string(), Held { rx, asked: now });
    }
    let held = reg.get_mut(source)?;
    held.asked = now;
    held.rx.frame()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_struct_matches_the_sdk_layout() {
        // NDIlib_video_frame_v2_t on 64-bit: 72 bytes.
        assert_eq!(std::mem::size_of::<VideoFrameV2>(), 72);
        assert_eq!(FOURCC_RGBA, 0x4142_4752);
    }

    #[test]
    fn a_missing_runtime_is_a_reason_not_a_panic() {
        // On a machine with the runtime this is `None`; either way it answers.
        match disabled_reason() {
            Some(why) => assert!(why.contains("NDI"), "{why}"),
            None => assert!(api().is_ok()),
        }
    }
}
