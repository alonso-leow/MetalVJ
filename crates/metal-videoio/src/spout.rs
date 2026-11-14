//! Spout (Windows): a sender through `SpoutLibrary.dll` (Spout2's C-compatible
//! build), opened at run time with `libloading`. Compiled only with
//! `target_os = "windows"` and the `spout` feature.
//!
//! `GetSpout()` returns a C++ `SPOUTLIBRARY*`; its methods are virtual, so
//! calling one means reading the object's vtable and the method's slot. The
//! slot table is [`SLOTS`]: it stays `None` until the slot order is copied from
//! `SpoutLibrary.h` on a Windows box. Until then [`open`] refuses with that
//! reason, so no unverified call ever reaches the DLL. The frames go through
//! `SendImage` (CPU RGBA into Spout's own DX11 texture), the same readback
//! path the pump already uses for every sink.

use std::ffi::{c_char, c_void, CString};

use crate::share::{sender_name, Share, ShareBackend};

const DLL: &str = "SpoutLibrary.dll";
/// `GL_RGBA`, the format `SendImage` takes from the pump's RGBA bytes.
const GL_RGBA: u32 = 0x1908;

/// Vtable slots of the methods used here: 0-based index of each virtual in
/// `SPOUTLIBRARY`'s declaration order (MSVC layout, x64, where the virtual
/// destructor takes one slot after `Release`). Source: Spout2 master
/// `SPOUTSDK/SpoutLibrary/SpoutLibrary.h`, fetched 2026-10-12. The file carries
/// no commit or tag comment, so the pin is the fetch date, not a commit; the
/// `NTDDI_WIN10_RS4` block (slots 91-96) is counted, as the DLL is built with a
/// Windows 10+ SDK. Recheck when bumping the vendored DLL.
#[derive(Clone, Copy)]
struct Slots {
    release_sender: usize, // ReleaseSender(DWORD dwMsec = 0)
    send_image: usize,     // SendImage(pixels, w, h, glFormat, bInvert)
    create_sender: usize,  // CreateSender(name, w, h, dwFormat = 0)
    create_opengl: usize,  // CreateOpenGL(HWND hwnd = nullptr)
    close_opengl: usize,   // CloseOpenGL()
    release: usize,        // Release(): library release, frees the object
}

const SLOTS: Option<Slots> = Some(Slots {
    release_sender: 2,
    send_image: 5,
    create_sender: 69,
    create_opengl: 97,
    close_opengl: 98,
    release: 174,
});

type GetSpout = unsafe extern "C" fn() -> *mut c_void;
type Release = unsafe extern "C" fn(this: *mut c_void);
type CreateSender = unsafe extern "C" fn(this: *mut c_void, name: *const c_char, w: u32, h: u32, format: u32) -> bool;
type ReleaseSender = unsafe extern "C" fn(this: *mut c_void, msec: u32);
type CreateOpenGL = unsafe extern "C" fn(this: *mut c_void, hwnd: *mut c_void) -> bool;
type CloseOpenGL = unsafe extern "C" fn(this: *mut c_void) -> bool;
type SendImage = unsafe extern "C" fn(this: *mut c_void, pixels: *const u8, w: u32, h: u32, gl_format: u32, invert: bool) -> bool;

fn load() -> Result<libloading::Library, String> {
    // SAFETY: the vendor's own library, opened by the name it ships under.
    unsafe { libloading::Library::new(DLL) }.map_err(|e| format!("{DLL} not found: {e}"))
}

/// Why Spout is off on this machine, `None` when it can send.
pub fn disabled_reason() -> Option<String> {
    if let Err(e) = load() {
        return Some(e);
    }
    if SLOTS.is_none() {
        return Some("Spout: the SpoutLibrary slot positions are not verified (pending a test on Windows)".into());
    }
    None
}

/// Vtable entry `slot` of the `SPOUTLIBRARY` object `this`.
///
/// # Safety
/// `this` must be a live `SPOUTLIBRARY*` from `GetSpout()`, and `slot` must be
/// a method slot of that class.
unsafe fn method<F: Copy>(this: *mut c_void, slot: usize) -> F {
    let vtable = *(this as *const *const usize);
    let entry = *vtable.add(slot);
    std::mem::transmute_copy::<usize, F>(&entry)
}

/// The DLL, its object and the slots, opened once for one sender.
struct Lib {
    _dll: libloading::Library,
    this: *mut c_void,
    slots: Slots,
}

impl Lib {
    fn open() -> Result<Lib, String> {
        let slots = SLOTS.ok_or_else(|| disabled_reason().unwrap_or_default())?;
        let dll = load()?;
        // SAFETY: `GetSpout` is the DLL's documented export, `()` -> object.
        let this = unsafe {
            let get: libloading::Symbol<GetSpout> =
                dll.get(b"GetSpout\0").map_err(|e| format!("GetSpout not found: {e}"))?;
            get()
        };
        if this.is_null() {
            return Err("GetSpout returned null".into());
        }
        // SendImage needs a GL context on this thread; with no host context the
        // library creates its own hidden one (header: "Not used if applications
        // already have an OpenGL context").
        // SAFETY: slot and signature from `SLOTS`; a null HWND asks for the hidden window.
        let ok = unsafe {
            let f: CreateOpenGL = method(this, slots.create_opengl);
            f(this, std::ptr::null_mut())
        };
        if !ok {
            return Err("Spout could not create the OpenGL context".into());
        }
        Ok(Lib { _dll: dll, this, slots })
    }
}

/// The Spout sender behind [`crate::Sink`].
pub struct SpoutBackend {
    lib: Lib,
}

// SAFETY: the object is only touched from the pump thread that owns this sink.
unsafe impl Send for SpoutBackend {}

impl ShareBackend for SpoutBackend {
    fn create(&mut self, name: &str, w: u32, h: u32) -> Result<(), String> {
        let c = CString::new(name).map_err(|_| "the Spout name contains a null byte".to_string())?;
        // SAFETY: slot and signature from `SLOTS`, `this` from `GetSpout`.
        let ok = unsafe {
            let f: CreateSender = method(self.lib.this, self.lib.slots.create_sender);
            f(self.lib.this, c.as_ptr(), w, h, 0)
        };
        if ok {
            Ok(())
        } else {
            Err(format!("Spout could not create the sender «{name}»"))
        }
    }

    fn publish(&mut self, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
        // SAFETY: as above; `rgba` holds `w × h × 4` bytes (checked by `Share`).
        let ok = unsafe {
            let f: SendImage = method(self.lib.this, self.lib.slots.send_image);
            f(self.lib.this, rgba.as_ptr(), w, h, GL_RGBA, false)
        };
        if ok {
            Ok(())
        } else {
            Err("Spout did not accept the frame".into())
        }
    }

    fn release(&mut self) {
        // SAFETY: as above; the sender exists only between create and release.
        unsafe {
            let f: ReleaseSender = method(self.lib.this, self.lib.slots.release_sender);
            f(self.lib.this, 0);
        }
    }
}

impl Drop for SpoutBackend {
    fn drop(&mut self) {
        // SAFETY: the slots come from `SLOTS`; `Release` is the last call on the object.
        unsafe {
            let close: CloseOpenGL = method(self.lib.this, self.lib.slots.close_opengl);
            close(self.lib.this);
            let f: Release = method(self.lib.this, self.lib.slots.release);
            f(self.lib.this);
        }
    }
}

/// Opens a Spout sender published as `target` (empty = `MetalVJ`).
pub fn open(target: &str) -> Result<Share<SpoutBackend>, String> {
    let lib = Lib::open()?;
    Ok(Share::new(sender_name(target, ""), SpoutBackend { lib }))
}
