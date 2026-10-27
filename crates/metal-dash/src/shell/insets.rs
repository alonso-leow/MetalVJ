//! System-bar insets for the phone shell.
//!
//! With `targetSdk 35` the NativeActivity window is edge-to-edge: the native
//! surface covers the status and navigation bars, so the header drew under
//! the clock and its taps missed. A Java-side inset listener never fired on
//! the emulator, so the bar heights are read here instead, over JNI, from
//! `Window.getDecorView().getRootWindowInsets()`. The shell reserves that
//! space with spacer panels above and below everything else.
//!
//! Desktop builds have no bars: both values are zero.

/// Status bar height at the top and navigation bar height at the bottom, in
/// physical pixels. `(0, 0)` off Android, or while the window has no insets
/// yet (before the first attach).
#[cfg(target_os = "android")]
pub fn system_bars_px() -> (f32, f32) {
    read_system_bars().unwrap_or((0.0, 0.0))
}

#[cfg(not(target_os = "android"))]
pub fn system_bars_px() -> (f32, f32) {
    (0.0, 0.0)
}

#[cfg(target_os = "android")]
fn read_system_bars() -> Option<(f32, f32)> {
    use jni::objects::{JObject, JValue};

    let ctx = ndk_context::android_context();
    let activity_ptr = super::window::android_activity_ptr();
    if activity_ptr.is_null() {
        return None;
    }
    // SAFETY: the VM comes from ndk-context and the activity from the
    // `AndroidApp` `android_main` stored; both live for the whole process.
    // ndk-context's own `context()` is the Application, which has no
    // `getWindow`, so the Activity pointer has to come from the app.
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;
    let activity = unsafe { JObject::from_raw(activity_ptr.cast()) };

    let window = env
        .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])
        .ok()?
        .l()
        .ok()?;
    let decor = env
        .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])
        .ok()?
        .l()
        .ok()?;
    let insets = env
        .call_method(
            &decor,
            "getRootWindowInsets",
            "()Landroid/view/WindowInsets;",
            &[],
        )
        .ok()?
        .l()
        .ok()?;
    if insets.is_null() {
        return None;
    }
    // WindowInsets.Type.systemBars() = statusBars | navigationBars = 0x7.
    let bars = env
        .call_method(
            &insets,
            "getInsets",
            "(I)Landroid/graphics/Insets;",
            &[JValue::Int(0x7)],
        )
        .ok()?
        .l()
        .ok()?;
    let top = env.get_field(&bars, "top", "I").ok()?.i().ok()?;
    let bottom = env.get_field(&bars, "bottom", "I").ok()?.i().ok()?;
    Some((top.max(0) as f32, bottom.max(0) as f32))
}
