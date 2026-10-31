#!/usr/bin/env bash
# Build the MetalVJ Android APK: cargo-ndk builds `metal-dash` as the
# `libmetal_dash.so` cdylib (feature `android`, desktop deps off), then the
# minimal Gradle project in `android/` packages it into an APK.
#
#   ./scripts/build-apk.sh              # debug APK (assembleDebug)
#   ./scripts/build-apk.sh --rust-only  # just the .so into jniLibs
#   ./scripts/build-apk.sh --install    # build, then adb install -r
#   ./scripts/build-apk.sh -t x86_64   # emulator build (google_apis x86_64 image)
#
# Environment (all optional):
#   ANDROID_HOME       SDK root              (default ~/Android/Sdk)
#   ANDROID_NDK_HOME   NDK root              (default newest under $ANDROID_HOME/ndk)
#   CARGO              cargo binary          (default ~/.cargo/bin/cargo, else PATH)
#   CARGO_TARGET_DIR   cargo target dir      (default ./target)
#   JAVA_HOME          JDK 17 or 21 for Gradle (auto-detected if unset or invalid:
#                      ~/.cache/fw-tools/jdk21, then /usr/lib/jvm/java-{21,17}-*)
#
# Rust toolchain prerequisites: rustup target aarch64-linux-android and
# `cargo install cargo-ndk`.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_DIR="$ROOT/android"
JNI_DIR="$ANDROID_DIR/app/src/main/jniLibs"
APK="$ANDROID_DIR/app/build/outputs/apk/debug/app-debug.apk"

RUST_ONLY=0
INSTALL=0
ABI="arm64-v8a"
while [ $# -gt 0 ]; do
  case "$1" in
    --rust-only) RUST_ONLY=1 ;;
    --install) INSTALL=1 ;;
    -t|--target) [ $# -ge 2 ] || { echo "build-apk: $1 needs an ABI" >&2; exit 2; }; ABI="$2"; shift ;;
    -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
case "$ABI" in
  arm64-v8a) RUST_TARGET=aarch64-linux-android ;;
  x86_64) RUST_TARGET=x86_64-linux-android ;;
  *) echo "build-apk: unknown target ABI '$ABI' (arm64-v8a or x86_64)" >&2; exit 2 ;;
esac

die() { echo "build-apk: $*" >&2; exit 1; }

# --- cargo -----------------------------------------------------------------
CARGO="${CARGO:-}"
if [ -z "$CARGO" ]; then
  if [ -x "$HOME/.cargo/bin/cargo" ]; then CARGO="$HOME/.cargo/bin/cargo"
  else CARGO="$(command -v cargo || true)"; fi
fi
[ -n "$CARGO" ] || die "cargo not found (set CARGO=...)"
"$CARGO" ndk --version >/dev/null 2>&1 || die "cargo-ndk missing: cargo install cargo-ndk"
rustup target list --installed 2>/dev/null | grep -qx "$RUST_TARGET" \
  || die "rust target missing: rustup target add $RUST_TARGET"

# --- Android SDK / NDK -------------------------------------------------------
ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
[ -d "$ANDROID_HOME/platforms" ] || die "no Android SDK at $ANDROID_HOME (set ANDROID_HOME)"
if [ -z "${ANDROID_NDK_HOME:-}" ]; then
  ANDROID_NDK_HOME="$(ls -d "$ANDROID_HOME"/ndk/*/ 2>/dev/null | sort -V | tail -n1 || true)"
  ANDROID_NDK_HOME="${ANDROID_NDK_HOME%/}"
fi
[ -d "${ANDROID_NDK_HOME:-}" ] || die "no NDK found (set ANDROID_NDK_HOME)"
export ANDROID_HOME ANDROID_NDK_HOME

# --- JDK for Gradle (17 or 21; Gradle 8.14 rejects 27) -----------------------
if [ -z "${JAVA_HOME:-}" ] || ! "$JAVA_HOME/bin/java" -version >/dev/null 2>&1; then
  JAVA_HOME=""
  for cand in "$HOME/.cache/fw-tools/jdk21/usr/lib/jvm/java-21-openjdk" \
              $(ls -d /usr/lib/jvm/java-21-* /usr/lib/jvm/java-17-* 2>/dev/null | sort -rV); do
    if [ -x "$cand/bin/java" ]; then JAVA_HOME="$cand"; break; fi
  done
fi
[ -n "$JAVA_HOME" ] && [ -x "$JAVA_HOME/bin/java" ] \
  || die "no JDK 17/21 found (set JAVA_HOME; looked in ~/.cache/fw-tools/jdk21 and /usr/lib/jvm/java-{21,17}-*)"
export JAVA_HOME
export PATH="$JAVA_HOME/bin:$PATH"

# --- Rust side: libmetal_dash.so into jniLibs/arm64-v8a ------------------------
echo "==> cargo ndk -t $ABI build -p metal-dash --release --no-default-features --features android"
(
  cd "$ROOT"
  "$CARGO" ndk -t "$ABI" -o "$JNI_DIR" build \
    -p metal-dash --lib --release --no-default-features --features android
)
SO="$JNI_DIR/$ABI/libmetal_dash.so"
[ -f "$SO" ] || die "expected $SO after cargo ndk"
# cpal's oboe backend is built against the shared C++ runtime (see metal-dash's
# Cargo.toml); the APK must carry it next to libmetal_dash.so.
STDCXX="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$RUST_TARGET/libc++_shared.so"
[ -f "$STDCXX" ] || die "NDK libc++_shared.so not found at $STDCXX"
cp "$STDCXX" "$JNI_DIR/$ABI/"

if [ "$RUST_ONLY" -eq 1 ]; then
  echo "built $SO"
  exit 0
fi

# --- Data: the read-only part of data/ ships as APK assets -------------------
# android_main copies these into the app's files dir on first launch (see
# stage_android_data in crates/metal-dash/src/lib.rs). The operator's own data
# (outputs, collections, shows, songs, the dataset) stays out of the APK.
ASSETS="$ANDROID_DIR/app/src/main/assets/data"
rm -rf "$ASSETS"
mkdir -p "$ASSETS"
# Device graphs are the per-fixture-head graphs the phone drives; the glob
# expands in the data dir so a new device-*.json rides along without an edit.
# Audio (data/audio) and digests are NOT packed. Audio is excluded by the
# metal-sync allowlist too (`audio/` never syncs), so a song cannot play on a
# phone until that changes; see docs/ANDROID.md.
# graphs/ is live data and absent from a clean checkout: pack what exists.
( cd "$ROOT/data" && tar -cf - schema.json keybinds.json outputs.json library shaders \
  router agent animations loops/templates models songs dataset \
  $(ls graphs/g.json graphs/device-*.json 2>/dev/null) ) | tar -C "$ASSETS" -xf -
{ git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo nogit; date +%s; } > "$ASSETS/build-stamp"
echo "==> staged $(du -sh "$ASSETS" | cut -f1) of data into assets"

# --- Gradle: APK -------------------------------------------------------------
[ -x "$ANDROID_DIR/gradlew" ] || die "android/gradlew missing"
echo "==> gradlew assembleDebug"
(cd "$ANDROID_DIR" && ./gradlew --no-daemon -PfwAbi="$ABI" assembleDebug)
[ -f "$APK" ] || die "APK not produced at $APK"
echo "built $APK"

ADB="$ANDROID_HOME/platform-tools/adb"
[ -x "$ADB" ] || ADB="$(command -v adb || echo adb)"
if [ "$INSTALL" -eq 1 ]; then
  "$ADB" install -r "$APK"
else
  echo "install with: $ADB install -r \"$APK\""
fi
