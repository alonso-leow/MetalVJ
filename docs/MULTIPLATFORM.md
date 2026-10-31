# Multiplatform builds

One app, several targets. The shell is `crates/metal-dash` (egui + HTTP server).
Desktop-only code (ffmpeg/ffplay spawns, `pactl`, `niri`, cpal specifics,
softbuffer, midir) sits behind features, so the same crate can be cut down for
other targets. Keep the split: a new platform adds a feature gate, not a fork.

| Target | Status | Entry point | Build |
| --- | --- | --- | --- |
| Linux desktop (`x86_64-unknown-linux-gnu`) | shipping | `metal-dash` bin | `cargo build -p metal-dash` |
| Android (`aarch64-linux-android`, `x86_64-linux-android` emulator) | APK builds, installs and renders the shell on the x86_64 emulator; frames are slow under SwiftShader | `android_main` in the `metal_dash` cdylib | `./scripts/build-apk.sh [-t x86_64]` |
| `wasm32-unknown-unknown` | sandbox builds (engine and WebGL2 painter; no shell panels) | `metal-dash` lib, `web.rs` (`#[wasm_bindgen(start)]`) | `./scripts/build-wasm.sh` |
| iOS (`aarch64-apple-ios`) | planned, needs a Mac | not yet written | `cargo-xcode` |
| `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin` | planned | the desktop bin | cross-compile per target |

## Android

**Crate.** `metal-dash` builds as `libfw_dash.so` (`[lib] name = "metal_dash"`,
`crate-type = ["cdylib", "rlib"]`). Feature `android` turns on
`android-activity` (NativeActivity backend) and exports `android_main`. The
desktop deps stay behind `desktop`, so the Android build uses
`--no-default-features --features android`.

**Packaging.** `android/` is a minimal Gradle project with one Java class:

- `app/src/main/java/com/metalvj/dash/MainActivity.java` extends
  `android.app.NativeActivity` and only requests runtime permissions in
  `onCreate`. The Rust side is untouched: `android_main` still runs on a
  native thread.
- `app/src/main/AndroidManifest.xml` declares `.MainActivity` with
  `android.app.lib_name = metal_dash`, so the platform loads `libfw_dash.so`
  from the APK's `lib/arm64-v8a/`.
- Permissions: `INTERNET` (install-time), `RECORD_AUDIO` and `CAMERA`
  (dangerous, requested at runtime in `MainActivity.onCreate`). The user
  sees the prompt on first launch; denial is not handled yet.
- `minSdk 26`, `targetSdk 35`, `compileSdk 35`, `buildToolsVersion 35.0.0`
  (the locally installed build-tools). `abiFilters` follows `-PfwAbi`
  (`arm64-v8a` default, `x86_64` for the emulator).
- `packaging.jniLibs.useLegacyPackaging = true`: `libfw_dash.so` and
  `libc++_shared.so` are extracted on disk (set in Gradle, not the deprecated
  manifest `extractNativeLibs`).
- `app/src/main/jniLibs/` is filled by cargo-ndk and git-ignored.

**Build.** `./scripts/build-apk.sh` runs, in order:

0. picks a JDK 17 or 21 and exports `JAVA_HOME`: an existing valid
   `JAVA_HOME` wins, else `~/.cache/fw-tools/jdk21`, else
   `/usr/lib/jvm/java-21-*`, then `java-17-*`. Fails with a message if none
   is found.

1. `cargo ndk -t <abi> -o android/app/src/main/jniLibs build -p metal-dash --lib
   --release --no-default-features --features android`, then copies NDK's
   `libc++_shared.so` next to it (cpal's oboe backend needs the shared C++
   runtime; the static one fails to link with NDK r27).
2. `./gradlew -PfwAbi=<abi> assembleDebug` in `android/`
3. prints `adb install -r android/app/build/outputs/apk/debug/app-debug.apk`
   (or installs with `--install`).

Flags: `--rust-only` stops after step 1; `-t x86_64` builds for the emulator
(`rustup target add x86_64-linux-android`).

**Audio.** Feature `android` includes `listen`, so cpal opens the default
input through AAudio (oboe) and the digester gets real capture. The ONNX tone
block stays off (no Android binary of ONNX Runtime).

**Child processes.** Every spawn goes through `metal_schema::platform::Command`.
On `target_os = "android"` it returns `io::ErrorKind::Unsupported` with
`"<program>: unavailable on this platform"` and never calls `execve`. Affected:
ffmpeg/ffplay/ffprobe, pactl, niri, python3, node, rustc, xdg-open, df, tar,
mkfifo. Callers already treat a failed spawn as "no result", so the shell runs
without them. Not covered: `metal-ml` (`curl`, tone feature only, not in the
Android graph) and `metal-render` example bins.

Every program the sources name, each answering "unavailable on this platform
(Android has no child processes)" there (a test computes this list from the
sources and checks it against this paragraph): `curl`, `df`, `ffmpeg`,
`ffplay`, `ffprobe`, `mkfifo`, `node`, `pactl`, `python3`, `rustc`, `sh`,
`sleep`, `tar`, `true`, `xdg-open` (`sh`, `sleep` and `true` are test helpers).
Programs chosen at run time (`Command::new(program)`: the MCP server command,
timecode tools, the script interpreter) take the same refusal.

**On-demand pulls.** A phone with no `songs/`, `models/` or device graph asks a
paired peer for it over `fw/frame/1`: `POST /song/pull?song=`,
`/graph/pull?graph=device-<id>`, `/model/pull?model=<name>` (status by `GET`).
Same listing, resumable stream and blake3 check for all three; only `audio/`,
`digest/`, `graphs/device-*` and `models/` are served.

**Video on Android.** Clip playback no longer needs ffmpeg; HAP encoding
still does.

- *Trait.* `metal_reel::clip::ClipDecoder` (`size`, `duration`, `seek`,
  `frame_at(t) -> Rgba`) is the one interface a clip's pictures come through.
  `metal_reel::clip::HapClip` implements it over a HAP proxy (pure Rust, every
  platform). `metal_reel::clip::media::MediaClip` implements it with
  AMediaExtractor + AMediaCodec, `target_os = "android"` only. The extractor
  gets the file as an fd (the media server cannot open the app's private
  files by path). The decoder renders into an `AImageReader` surface
  (`YUV_420_888`, consumer usage `0x903`, the one the emulator's decoder asks
  for), and each picture is read back as an image with its own strides and
  crop. Target-only deps of `metal-reel`: `ndk` (`media`, `api-level-24/26`) and
  `ndk-sys`. `AMediaCodec_getOutputImage` is not in the NDK.
- *Decoder fallback.* The default decoder is tried first. The emulator's
  `c2.goldfish.h264` decoder takes a few samples and never returns a picture;
  after five seconds of polls with none, the clip is decoded again on
  `c2.android.avc.decoder` by name. Native decodes run one at a time.
- *Length.* A container without a duration is decoded to end-of-stream
  (`MediaClip::sample_all`), and its frames are counted, not sized in advance.
- *Order in `metal-dash` playout* (`call/author/playout.rs`, `decoded()`): a HAP
  proxy that exists is read first, as before. Without one, on Android the clip
  is decoded natively and sampled at the digest rate into the same JPEG list
  the desktop ffmpeg MJPEG pipe fills, so the preview and its routes are
  unchanged. Desktop still runs ffmpeg.
- *Conversion.* `metal_reel::clip::Yuv420::to_rgba` takes a codec output buffer
  (I420 colour 19 or NV12 colour 21, with stride, slice-height and crop) and
  converts to RGBA at the preview width (BT.709 at 720 p and up, BT.601
  below, limited range). Pure Rust, unit-tested on the host.
- *HAP on Android.* Proxies are written on desktop (`encode_hap` needs the
  ffmpeg `hap` encoder) and copied to the device; the reader is
  `metal_reel::hap` (snappy + BC1/BC3 CPU decode), pure Rust, so a synced proxy
  plays with no codec at all.
- *Gaps.* No HAP encoding on Android. No JPEG ladder, so no scene cut list or
  posters for a clip decoded natively (the scene editor shows one passage).
  Decoders that report a vendor or flexible colour format with no 4:2:0 layout
  fail with a message and the clip stays dark. Frame-at-time is sequential
  with a seek only after a jump longer than 2 s; not profiled on a device yet.
  A clip with no container duration gets no frames.

**Emulator.** `sdkmanager` installs `emulator` and
`system-images;android-35;google_apis;x86_64`. Create the AVD with
`avdmanager create avd -n fwtest -k "system-images;android-35;google_apis;x86_64" -d pixel_6`,
boot with `emulator -avd fwtest -no-window -no-audio -gpu swiftshader_indirect`
(KVM is available on the dev host). Verified: the APK installs, the activity
starts, `libfw_dash.so` loads, the renderers start, the Oboe input stream opens.

**Data (startup).** The shell reads `data/...` through literal relative paths
(about 30 of them in `metal-dash`'s author code). `build-apk.sh` packs the
read-only subset of `data/` as APK assets (`schema.json`, `keybinds.json`,
`outputs.json`, `library/`, `shaders/`, `router/`, `agent/`, `animations/`,
`loops/templates/`, `models/`, `songs/`, `dataset/`, `graphs/g.json` and
`graphs/device-*.json`; about 7.2 MB) into
`android/app/src/main/assets/data/`, gitignored. `android_main` calls
`stage_android_data` (`lib.rs`): when `data/build-stamp` in the APK differs
from the staged copy in `<files>/data`, it copies the assets there, then sets
the cwd to `<files>` and `output::set_data_root` to `<files>/data`. The
literal `data/...` paths and `data_root()` then agree, and the writable files
(outputs, graphs, chats) live in the app's internal storage. The stamp is not
a dotfile, because aapt drops dotfiles from the APK.

**Verified (2026-10-09, x86_64 emulator, Android 35), video.** A 2 s and a 4 s
H.264 clip (generated with ffmpeg, pushed into the app's files dir) decode
through `/author/video/seek` and `frame.jpg`: 16 and 32 frames at the digest
rate. The 4 s clip (testsrc, then smptebars) cuts at 2.0 s with a poster per
scene. The decoded second scene is `docs/shots/android-video.png`. This was
driven over HTTP with adb forwarding; loading the clip as a layer in the UI
was not done.

**Verified again (2026-10-31, x86_64 emulator, Android 35), native H.264.** The debug APK built with
`scripts/build-apk.sh -t x86_64` (cargo-ndk; no release keystore) installs, the shell starts
(`108 renderers on Vulkan`), and a 2 s 640x360 H.264 clip (ffmpeg testsrc, pushed to the app's
files dir) decodes through `MediaClip` (AMediaExtractor + AMediaCodec): `/author/video/seek`
reports `decoded: 16, done: true` after the five-second goldfish-decoder fallback, and
`frame.jpg` returns a 480x270 picture (`docs/shots/android-h264-frame.jpg`; the device screen is
`docs/shots/android-h264-screen.png`). Fixes that build needed: the script and gradle still named
`libfw_dash.so` (the lib is `libmetal_dash.so`), `call/render.rs` reached the desktop-only
`remote_layers`, `window.rs` a missing `software_gpu` path, `tar` failed on the absent live
`graphs/`, and `metal-geom`'s Manifold CSG (host C++ build, `libstdc++` symbols) made
`libmetal_dash.so` unloadable on the device; on Android `csg::boolean` now answers
`SolverNotAvailable`.

**Verified (2026-10-09, x86_64 emulator, Android 35).** Startup reaches
`42 renderers on the GPU`, the Wgpu paint backend, the digest on cpal, and the
HTTP/OSC listeners. The shell is on screen: `docs/shots/android-shell.png`.
No panic in logcat.

**Software GPU.** The window detects a software adapter (`DeviceType::Cpu`
or a SwiftShader/llvmpipe/lavapipe name) on open and sets
`frames::software_gpu()`. The frame gap is the same 60 fps cap as everywhere
(`frames::LIVE_GAP`); the preview asks for pictures at most 320 px wide, and
lanes playback repaints through `frames::animate`. Under the emulator the perf line
reads `fps 2.0, frame p50 603 ms, paint 575 ms`, and no ANR appeared in 60 s.
Logcat shows the app's stdout/stderr under the tag `metal-dash`, including the
`perf:` lines (every 120 frames on Android).

**Sync on the phone (2026-10-10).** `android_main` starts `metal-sync` in-process
(`start_sync_node` in `lib.rs`, `metal_sync::node::spawn_thread`): data root
`<files>/data`, keys and iroh stores under `<files>/metal-sync`, control on
`127.0.0.1:7901`. The node's body moved from `metal-sync`'s `main` into
`metal_sync::node::run` so both hosts share it. Verified on the x86_64 emulator:
`curl 127.0.0.1:7901/status` (adb forward) answers with a node id and the
document namespace; logcat shows `control on 127.0.0.1:7901`. Not verified:
pairing with a second device, the song pull over `fw/frame/1`.

**Song audio on Android (2026-10-10).** `/author/play` no longer spawns ffplay
on Android: `call/author/audio_out.rs` decodes the file with
`metal_listen::file::decode_pcm` (symphonia) and streams it through a cpal
output (AAudio via oboe). The stream lives on its own thread (oboe's stream is
not `Send`). `GET /author/play` returns `{playing, at, native}` from the
stream's cursor. Verified on the emulator: logcat shows the `OboeAudio` output
open and `AAudioStream_requestStart` returns 0, the `metal-dash` line
`audio: song from 3.000s on default`, and `at` advances in real time (3.0 to
5.2 to 7.2 s over two polls). Desktop: `cargo build -p metal-dash -F native-audio`
(ffplay stays the default there). Not done: the lanes playhead still runs on
the wall clock, and no latency compensation.

**Remaining gaps.**
- Paint is about 575 ms per frame under SwiftShader, which is the cost that
  remains. The software reductions bound the rate but did not cut paint time
  much. Judge on real GPU hardware.
- Idle: on a software GPU `Wake::Idle` parks the loop (no heartbeat). Desktop
  keeps the 10 Hz heartbeat because its live readouts are drawn from it.
  Every shell repaint request goes through `frames::repaint` /
  `frames::repaint_after`, which log the call site under `FW_REPAINT_TRACE=1`
  (or a `repaint-trace` file in the files dir on Android).
- Song audio is not in the document (`audio/` and `digest/` stay out of the
  allowlist). A node that opens a song without its audio pulls the bundle from
  a paired peer over `fw/frame/1` (`metal-sync`'s `pull.rs`, `GET/POST
  /song/pull`), streamed and resumable, and the lanes panel shows its progress.
  Song playback on Android is native (see "Song audio on Android"); the lanes
  playhead is not yet driven by the audio clock.
- Clip-as-layer through the UI on the emulator: not verified. The phone
  header's grafo/lanes picker is one menu button (`ver: grafo ▾`) placed next
  to the wordmark, and a kittest checks it is in the viewport at phone sizes.
  On the emulator, adb taps on the header row did not open it (body taps work);
  the header is drawn under the status bar, so the top inset is the suspect.
  Screenshot of the header: `docs/shots/android-header.png`.
- Lanes playhead: follows the audio clock while native audio plays. Measured
  on the emulator (see "Lanes playhead vs. audio clock" below). Pairing and the song pull end to end: not run.
- Video on the emulator: verified 2026-10-09, see "Verified" above.
- The arm64-v8a build is produced by `build-apk.sh` (its Rust side is a
  release `.so`; the APK is debug). No release signing is wired.
- Video (ffmpeg) and the desktop-only process spawns are still absent.

**Environment.** Rust needs `rustup target add aarch64-linux-android` and
`cargo install cargo-ndk`. The SDK is `~/Android/Sdk` by default
(`ANDROID_HOME`), NDK `27.2.12479018` (or the newest under `ndk/`,
`ANDROID_NDK_HOME`). The Gradle wrapper needs a JDK (17 or 21 works with
Gradle 8.14; the system JDK 27 is too new for it). The script auto-detects
one (see step 0); `JAVA_HOME` overrides it.

To build only the APK without the script (placeholder `.so` must already be
in `jniLibs/arm64-v8a/`): `cd android && JAVA_HOME=<jdk21> ./gradlew
:app:assembleDebug`. Check the activity with
`$ANDROID_HOME/build-tools/35.0.0/aapt2 dump xmltree <apk> --file
AndroidManifest.xml`.

**Install and run.** Plug in a phone with USB debugging, then
`adb install -r <apk>` and launch "MetalVJ". Logs: `adb logcat -s RustStdoutStderr`
(Rust's stderr goes there, not under the `metal_dash` tag).

## Web (`wasm32`)

One engine, two targets. The browser build is `metal-dash` as a library
(`--lib --target wasm32-unknown-unknown --no-default-features`), with
`web.rs` as its eframe entry point. What it shares with the desktop, and what
it swaps:

- **Same:** `State::new`, the migrations, `Shared`, `render_device` for the
  preview. `metal-perform`, `metal-graph`, `metal-config`, `metal-reel`, `metal-router` and
  `metal-listen` (decode) compile unchanged.
- **Files:** every crate that touches the disk goes through `metal_vfs`. Native it
  re-exports `std::fs`; on wasm it is the in-memory tree, seeded from
  `web/wasm/data` and mirrored to `localStorage` through the write hook.
  `metal_vfs --features mem` runs the same tree on native (its tests: `cargo test
  -p metal-vfs`).
- **Time:** `web_time` instead of `std::time::Instant` (which panics on wasm).
- **Painter:** `metal-render` builds for wasm32 with the native window stack
  behind `cfg(not(wasm32))`. `Model`/`Paint` drop their `Send` supertrait on
  wasm only (`MaybeSend`/`PaintBound`), because wgpu's web handles are not
  `Send` and the page is single-threaded. `metal_render::gpu::web()` takes WebGPU
  when `navigator.gpu` exists, else a WebGL2 adapter on a detached canvas
  (`Backend::Gl`, which asks for the WebGL2 limits). Two things differ there:
  WebGL2 has no compute, so `Model::needs_compute` models (dla, solvent,
  turing, terrario) are drawn by `metal_perform::stand::Stand` per model
  (`Gpu(renderer, stand)` and `Paint::painter_of`); and a GL device runs its
  fences as `AutoFinish`, because a browser cannot block, so the readback's
  `poll(Wait)` returns. Without a usable adapter the whole suite is the CPU
  stand-in.
- **Shell:** `shell/` compiles for wasm32 (egui is a wasm dependency).
  `shell/window.rs` (winit) is the only piece gated out. `web.rs` builds the
  door (`call::Door`) over the browser's `Shared` and the `Author` stub, and
  hosts `Shell::ui` through eframe's WebRunner. The shell's background threads
  (preview fetcher, read refresher) are not started in the browser.
- **Panics:** a wasm panic only traps. `web.rs` installs a hook that writes the
  message to the console, so a failure on the page is named.
- **Off:** HTTP, the OSC reader thread, the AI harness, Bluetooth, process
  spawns, the winit window, the `/author/*` routes.

Build and check: `scripts/build-wasm.sh`, then serve `web/wasm/` over HTTP and
open `index.html`. Captures are headless Firefox: `docs/shots/wasm.png` (the
engine's two layers drawn on WebGL2) and `docs/shots/wasm-shell.png` (the shell
after a click on `gen-0`, Properties filled in).

## iOS

Needs a Mac for signing and `cargo-xcode`. Not started.

## Other desktop targets

`aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`:
the desktop binary with the default features. The `--low-power` GLES backend
(`glow` feature) and local `memfd` frame transport are also planned.

## Frames between machines: QOI budget

A remote layer's frame travels raw RGBA on a direct LAN and QOI on relay
(`remote_layers::choose_fmt`). The serving peer encodes in 8 independent
bands (`qoix`). Measured 2026-10-30 on a quiet box (i5-8300H laptop, load < 0.1
before the run), release build,
`cargo test --release -p metal-dash --lib qoi::tests::bench_720p -- --ignored --nocapture`:

| 720p frame | qoix encode | qoix decode | bytes | single-band qoi encode | jpeg q82 encode |
|---|---|---|---|---|---|
| gradient | 2.33 ms | 2.52 ms | 493 768 | 5.06 ms | 8.78 ms |
| noisy (worst case) | 3.06 ms | 3.74 ms | 1 114 006 | 6.87 ms | 12.75 ms |

The banded encoder holds the < 5 ms budget for both frames. Still open (need
machines): the Pi LAN run with `scripts/frames-perf-log.sh` and the relay run
phone ↔ desktop.

## Rules

- Platform code goes behind a feature or `cfg(target_os)`, never a parallel
  copy of the shell.
- Android keeps Java to the minimum: `MainActivity` exists only for platform
  APIs that need a Java call (runtime permissions). Rust code stays in
  `android_main`, which NativeActivity runs on a native thread.
- Generated artifacts (`jniLibs/`, `android/app/build/`) are never committed.

**System bars.** With `targetSdk 35` the window is edge-to-edge, so the native
surface drew under the status bar. `shell/insets.rs` reads the status and
navigation bar heights over JNI (`Window.getRootWindowInsets`, via the
activity pointer `shell/window.rs` keeps) and the shell adds spacer panels
for them. A Java-side inset listener never fired on the emulator, so it was
dropped. Verified on emulator-5554: header below the clock, picker opens on
tap (`docs/shots/android-header.png`).

**Clip as a layer.** Library long-press on a video card → "add as layer"
puts the clip on capa 1 (`docs/shots/android-clip-layer.png`).

**Audio clock.** cpal's playback stamp on the emulator reported ~376 s of
output latency, which pinned `audio_out::position()` at 0. A latency over one
second now falls back to the callback's buffer length.

**Pairing and song pull (emulator).** Host `metal-sync` (port 7911, temp data
tree, digested 12 s tone) sent a pair request to the device node
(`/pair/request`); the device showed a code (`/pair/pending`); the host
confirmed it (`/pair/confirm`). Both report `paired: true` in one namespace.
`POST /author/song/pull` on the device then fetched 2 files. Forwards:
`adb forward tcp:17900 tcp:7900`, `tcp:17901 tcp:7901`.

**Lanes playhead vs. audio clock (emulator-5554, 2026-10-09).** Lanes play
on an undigested song (both songs on the device are undigested, so the
transport used to refuse with a note off-screen; it now plays on the audio
clock). Read from `GET /author/play`: `at` (the audio clock) and `playhead_s`
(the lanes playhead the shell published) in the same reply, once a second.
- Run 1 (`tone`, 12 s): 10 playing samples over 8.4 s; lag mean 0.12 s, range
  0.03 to 0.25 s; playhead span 8.47 s vs audio span 8.42 s.
- Run 2 (`song`): 31 samples over 28 s; lag mean 0.14 s, range -0.03 to
  0.32 s; span drift 0.10 s over 28 s (0.36 %).
- The playhead stops with the audio at the song's end (`playing` false).
- Two causes fixed on the way: the read cache answered `GET /author/play`
  from its last reply for up to 250 ms (and stale while refreshing), so the
  clock is now read live; and `audio_out::position()` reads the callback's
  anchor, unchanged.

**Sync port.** `FW_SYNC_PORT` (default 7901) sets metal-sync's control port
for the host metal-dash (`call/author/playout.rs`, `monitor.rs`) and the
in-process node on the phone. The emulator forwards stay on 17900/17901.

## Browser: the preview painter in a Web Worker

`web/wasm/worker.js` runs the same wasm module as the page. At startup, once
(the backend rule), `web_worker::try_worker` checks for `Worker` and
`OffscreenCanvas`, transfers a canvas to the worker
(`transferControlToOffscreen`), and waits for the worker's answer about its
device (WebGPU, else WebGL2, on that canvas: `metal_render::gpu::web_worker`).
All present: the page's painter is a `WorkerPaint`, whose pixel draws go over
`postMessage` and come back as frames landed in a mailbox (one request in flight
per model and size; the stand-in's figure answers until the first frame lands,
the newest landed frame after). Any piece missing, or no answer in 15 s: the
page paints inline as before. The desktop's preview thread (`shell/frames.rs`)
is unchanged and native only.

The wire and the fallback decision are in `metal_perform::worker_proto` and are
tested natively (header and float round trip, reply encoding, mailbox, mode
choice). `cargo check` for wasm32 and `scripts/build-wasm.sh` pass.

**Not run: a real browser.** Needs a person: that the worker boots a device on
the transferred canvas (WebGPU and WebGL2 in a worker), the `OffscreenCanvas`
transfer, the non-blocking readback (`GpuCx::readback_with` polled with a yield),
the frame cadence under load, and the models that need compute shaders (they
reply "no frame" and stay on the stand-in's figure).
