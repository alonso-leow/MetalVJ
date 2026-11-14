#!/usr/bin/env bash
# package-linux.sh -- cut a Linux AppImage for MetalVJ.
#
#   scripts/package-linux.sh            # build + package
#   scripts/package-linux.sh --skip-build  # reuse target/release/metal-dash
#
# Produces: dist/MetalVJ-<version>-linux-x86_64.AppImage
# and prints its sha256 (the updater verifies the download against it).
#
# Why this runs in CI and not on the maintainers' Windows box: the desktop
# binary links ALSA (feature `listen`), X11/Wayland (winit/glutin) and the GL
# stack, so it can only be built by a Linux host with those -dev packages
# installed. `.github/workflows/release.yml` (ubuntu-24.04) is the delivery
# path; this script is what that job runs.
#
# Tooling (linuxdeploy + appimagetool) is downloaded at build time into a
# cache dir, never vendored into the repo. `appimagetool --appimage-extract-
# and-run` avoids needing FUSE on the runner.
#
# <version> is read from [workspace.package] version in Cargo.toml -- never
# hardcoded here, so it matches scripts/package-windows.ps1.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

die() { echo "package-linux: $*" >&2; exit 1; }

skip_build=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) skip_build=1 ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) die "unknown option: $arg" ;;
  esac
done

# --- version ----------------------------------------------------------------
# Only the [workspace.package] table, so no dependency's version is mistaken
# for ours.
version="$(awk '
  /^\[workspace\.package\]/ { inpkg = 1; next }
  /^\[/ { inpkg = 0 }
  inpkg && $1 == "version" { gsub(/[" ]/, "", $3); print $3; exit }
' Cargo.toml)"
[ -n "$version" ] || die "could not read version from [workspace.package] in Cargo.toml"
echo "package-linux: version $version"

name="MetalVJ-$version-linux-x86_64.AppImage"
out="$root/dist/$name"
mkdir -p "$root/dist"
# Stale output must not survive a failed rebuild and be hashed as this run's.
rm -f "$out"

# --- toolchain --------------------------------------------------------------
command -v cargo >/dev/null 2>&1 || die "cargo not found on PATH (install Rust: https://rustup.rs)"
command -v pkg-config >/dev/null 2>&1 || die "pkg-config not found: install pkg-config libasound2-dev libudev-dev libssl-dev"

# The desktop build pulls ALSA through cpal (feature `listen`). Fail with the
# remedy rather than a rustc wall of text 300 crates in.
if ! pkg-config --exists alsa; then
  die "ALSA development headers missing: sudo apt-get install libasound2-dev"
fi

if [ "$skip_build" -eq 0 ]; then
  cargo build -p metal-dash --release
fi
bin="$root/target/release/metal-dash"
[ -x "$bin" ] || die "expected binary at $bin (drop --skip-build to build it)"

# --- tooling ----------------------------------------------------------------
# Pinned to the AppImageKit/linuxdeploy `continuous` builds. URLs and the
# SHA-256 below were taken from the published assets when this script was
# written (both 200, digests computed locally); neither project publishes a
# `.sha256` asset, so the digests are pinned *here*.
#
#   appimagetool  https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
#   linuxdeploy   https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
#
# Because `continuous` moves, a mismatch means upstream shipped a new build:
# re-run with METALVJ_REPIN_APPIMAGE_TOOLS=1 to print the current digests, and
# update the two constants. Never vendor these binaries into the repository.
#
# A tool already on PATH is used as-is; the download is only for CI/fresh hosts.
cache="${METALVJ_APPDIR_CACHE:-$root/packaging/.cache}"
mkdir -p "$cache"

appimagetool_sha256="95cbe7cce9717fce90c484e34052ee7c7f1d7635b33c12525b4776826a7d29b6"
linuxdeploy_sha256="8aea8da0f7f7039d2a2cecb14657d752a222a5e1d3825caeef186c82f751cdd1"

# Download $url to $dest and verify it against $want (hex sha256). A mismatch
# is fatal; there is no "unverified" mode because an AppImage toolchain that
# silently changed is exactly what this check exists to catch.
fetch() {
  local url="$1" dest="$2" want="$3"
  [ -x "$dest" ] && return 0
  echo "package-linux: downloading $url"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$dest.part" "$url"
  else
    wget -q -O "$dest.part" "$url"
  fi
  local have
  have="$(sha256sum "$dest.part" | awk '{print $1}')"
  if [ "$have" != "$want" ]; then
    rm -f "$dest.part"
    if [ "${METALVJ_REPIN_APPIMAGE_TOOLS:-0}" = "1" ]; then
      echo "package-linux: current sha256 for $(basename "$dest") is $have" >&2
      exit 2
    fi
    die "checksum mismatch for $(basename "$dest"): want $want got $have (upstream moved; see METALVJ_REPIN_APPIMAGE_TOOLS)"
  fi
  echo "package-linux: checksum ok for $(basename "$dest")"
  mv "$dest.part" "$dest"
  chmod +x "$dest"
}

linuxdeploy_url="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
appimagetool_url="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"

if command -v linuxdeploy >/dev/null 2>&1; then
  linuxdeploy="$(command -v linuxdeploy)"
else
  linuxdeploy="$cache/linuxdeploy-x86_64.AppImage"
  fetch "$linuxdeploy_url" "$linuxdeploy" "$linuxdeploy_sha256"
fi
if command -v appimagetool >/dev/null 2>&1; then
  appimagetool="$(command -v appimagetool)"
else
  appimagetool="$cache/appimagetool-x86_64.AppImage"
  fetch "$appimagetool_url" "$appimagetool" "$appimagetool_sha256"
fi

export ARCH=x86_64
export APPIMAGE_EXTRACT_AND_RUN=1

# --- AppDir -----------------------------------------------------------------
# packaging/AppDir is generated, gitignored, and rebuilt from scratch so a
# removed file never lingers in the image.
appdir="$root/packaging/AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/icons/hicolor/256x256/apps"

install -m 0755 "$bin" "$appdir/usr/bin/metal-dash"
install -m 0644 "$root/packaging/MetalVJ.desktop" "$appdir/MetalVJ.desktop"
install -m 0644 "$root/packaging/icons/MetalVJ.png" "$appdir/MetalVJ.png"
install -m 0644 "$root/packaging/icons/MetalVJ.png" \
  "$appdir/usr/share/icons/hicolor/256x256/apps/MetalVJ.png"
install -m 0755 "$root/packaging/AppRun" "$appdir/AppRun"

# linuxdeploy copies the shared libraries the binary needs (ALSA, X11/Wayland,
# libGL, libEGL, ...) into the AppDir. --output appimage then calls
# appimagetool itself; we do it by hand below so --appimage-extract-and-run is
# guaranteed even when linuxdeploy found a FUSE-less box.
"$linuxdeploy" --appdir "$appdir" \
  --executable "$appdir/usr/bin/metal-dash" \
  --desktop-file "$appdir/MetalVJ.desktop" \
  --icon-file "$appdir/MetalVJ.png" \
  --plugin gtk 2>/dev/null || \
  "$linuxdeploy" --appdir "$appdir" \
    --executable "$appdir/usr/bin/metal-dash" \
    --desktop-file "$appdir/MetalVJ.desktop" \
    --icon-file "$appdir/MetalVJ.png"

# linuxdeploy rewrites the desktop file; make sure the AppRun it would have
# written did not replace ours (ours sets the working directory the app
# expects and forwards argv).
install -m 0755 "$root/packaging/AppRun" "$appdir/AppRun"

echo "package-linux: building AppImage"
"$appimagetool" --appimage-extract-and-run --no-appstream "$appdir" "$out"

[ -f "$out" ] || die "appimagetool did not produce $out"
chmod +x "$out"

# --- report -----------------------------------------------------------------
size="$(wc -c < "$out" | tr -d ' ')"
hash="$(sha256sum "$out" | awk '{print $1}')"
echo ""
echo "artifact : $out"
echo "size     : $size bytes"
echo "sha256   : $hash"
echo ""
# Machine-readable last line: CI greps for it, humans read the block above.
echo "SHA256 $hash  $name"
