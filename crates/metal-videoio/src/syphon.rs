//! Syphon (macOS): a `SyphonMetalServer` over an `MTLTexture`, shared with
//! other programs through IOSurface. Compiled only with `target_os = "macos"`
//! and the `syphon` feature.
//!
//! `Syphon.framework` is loaded at run time (a missing framework is a disabled
//! mode with its path in the reason, never a build failure). Every Objective-C
//! call goes through `objc2::msg_send!` on raw object pointers, and the
//! server, queue, device and texture are released by hand in [`Drop`]. The
//! frame is uploaded with `replaceRegion` into an `RGBA8Unorm` texture and
//! published with `publishFrameTexture:onCommandBuffer:imageRegion:flipped:`.
//!
//! Not verified on a Mac: the selectors and struct encodings follow Syphon's
//! Metal server header but were compiled for `aarch64-apple-darwin`, not run.

use std::ffi::c_void;
use std::sync::OnceLock;

use objc2::encode::{Encode, Encoding};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::NSString;

use crate::share::{sender_name, Share, ShareBackend};

const FRAMEWORK: &str = "/Library/Frameworks/Syphon.framework/Syphon";
/// `MTLPixelFormatRGBA8Unorm`.
const PIXEL_RGBA8: usize = 70;

#[link(name = "Metal", kind = "framework")]
extern "C" {
    fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MTLOrigin {
    x: usize,
    y: usize,
    z: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MTLSize {
    width: usize,
    height: usize,
    depth: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MTLRegion {
    origin: MTLOrigin,
    size: MTLSize,
}

// SAFETY: field layout matches the Metal headers (three NSUInteger-sized
// fields, then the nested origin and size).
unsafe impl Encode for MTLOrigin {
    const ENCODING: Encoding = Encoding::Struct("MTLOrigin", &[usize::ENCODING, usize::ENCODING, usize::ENCODING]);
}
unsafe impl Encode for MTLSize {
    const ENCODING: Encoding = Encoding::Struct("MTLSize", &[usize::ENCODING, usize::ENCODING, usize::ENCODING]);
}
// SAFETY: `CGRect` as the CoreGraphics headers lay it out (doubles).
unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}
unsafe impl Encode for MTLRegion {
    const ENCODING: Encoding = Encoding::Struct("MTLRegion", &[MTLOrigin::ENCODING, MTLSize::ENCODING]);
}

/// The framework, loaded once for the process and never unloaded (the
/// classes it registers stay live).
fn framework() -> Result<(), String> {
    static LOADED: OnceLock<Result<(), String>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            // SAFETY: the vendor's own framework binary, at its installed path.
            match unsafe { libloading::Library::new(FRAMEWORK) } {
                Ok(lib) => {
                    std::mem::forget(lib);
                    Ok(())
                }
                Err(e) => Err(format!("Syphon.framework not loaded ({FRAMEWORK}): {e}")),
            }
        })
        .clone()
}

/// Why Syphon is off on this machine, `None` when it can send.
pub fn disabled_reason() -> Option<String> {
    framework().err()
}

/// The Syphon server and the Metal objects it publishes from.
pub struct SyphonBackend {
    device: *mut AnyObject,
    queue: *mut AnyObject,
    server: *mut AnyObject,
    texture: *mut AnyObject,
}

// SAFETY: the objects are only messaged from the pump thread that owns the sink.
unsafe impl Send for SyphonBackend {}

impl SyphonBackend {
    fn new() -> SyphonBackend {
        SyphonBackend {
            device: std::ptr::null_mut(),
            queue: std::ptr::null_mut(),
            server: std::ptr::null_mut(),
            texture: std::ptr::null_mut(),
        }
    }
}

impl ShareBackend for SyphonBackend {
    fn create(&mut self, name: &str, w: u32, h: u32) -> Result<(), String> {
        framework()?;
        // SAFETY: plain Objective-C messages on objects this backend creates
        // and releases; every argument type is the one the selector declares.
        unsafe {
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                return Err("Metal finds no device".into());
            }
            self.device = device;
            let queue: *mut AnyObject = msg_send![device, newCommandQueue];
            if queue.is_null() {
                return Err("Metal cannot create the command queue".into());
            }
            self.queue = queue;

            let cls = AnyClass::get(c"SyphonMetalServer").ok_or("SyphonMetalServer is not in Syphon.framework")?;
            let ns_name = NSString::from_str(name);
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            let server: *mut AnyObject = msg_send![alloc, initWithName: &*ns_name, device: device, options: std::ptr::null::<AnyObject>()];
            if server.is_null() {
                return Err(format!("Syphon could not create the server «{name}»"));
            }
            self.server = server;

            let desc_cls = AnyClass::get(c"MTLTextureDescriptor").ok_or("MTLTextureDescriptor does not exist")?;
            let desc: *mut AnyObject = msg_send![
                desc_cls,
                texture2DDescriptorWithPixelFormat: PIXEL_RGBA8,
                width: w as usize,
                height: h as usize,
                mipmapped: false
            ];
            let texture: *mut AnyObject = msg_send![device, newTextureWithDescriptor: desc];
            if texture.is_null() {
                return Err(format!("Metal cannot create a {w}×{h} texture"));
            }
            self.texture = texture;
        }
        Ok(())
    }

    fn publish(&mut self, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize { width: w as usize, height: h as usize, depth: 1 },
        };
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: w as f64, height: h as f64 },
        };
        // SAFETY: `rgba` holds `w × h × 4` bytes (checked by `Share`); the
        // texture, queue and server were created by `create` and are live.
        unsafe {
            let _: () = msg_send![
                self.texture,
                replaceRegion: region,
                mipmapLevel: 0usize,
                withBytes: rgba.as_ptr() as *const c_void,
                bytesPerRow: w as usize * 4
            ];
            let cb: *mut AnyObject = msg_send![self.queue, commandBuffer];
            if cb.is_null() {
                return Err("Metal cannot create the command buffer".into());
            }
            let _: () = msg_send![
                self.server,
                publishFrameTexture: self.texture,
                onCommandBuffer: cb,
                imageRegion: rect,
                flipped: false
            ];
            let _: () = msg_send![cb, commit];
        }
        Ok(())
    }

    fn release(&mut self) {
        // SAFETY: each pointer was retained by `create` (alloc/init and the
        // `new…` selectors return +1) and is released exactly once here.
        unsafe {
            if !self.server.is_null() {
                let _: () = msg_send![self.server, stop];
                let _: () = msg_send![self.server, release];
                self.server = std::ptr::null_mut();
            }
            for obj in [&mut self.texture, &mut self.queue, &mut self.device] {
                if !obj.is_null() {
                    let _: () = msg_send![*obj, release];
                    *obj = std::ptr::null_mut();
                }
            }
        }
    }
}

impl Drop for SyphonBackend {
    fn drop(&mut self) {
        self.release();
    }
}

/// Opens a Syphon server published as `target` (empty = `MetalVJ`).
pub fn open(target: &str) -> Result<Share<SyphonBackend>, String> {
    framework()?;
    Ok(Share::new(sender_name(target, ""), SyphonBackend::new()))
}
