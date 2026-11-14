#!/usr/bin/env bash
# package.sh -- pick the packaging script for this host.
#
#   scripts/package.sh          # auto-detect (Windows -> PowerShell, Linux -> AppImage)
#   scripts/package.sh windows  # force the Windows .exe build
#   scripts/package.sh linux    # force the AppImage build (needs a Linux host + tooling)
#
# A thin dispatcher, not a build system: both scripts read the version from
# Cargo.toml and print `SHA256 <hash>  <name>` as their last line, which is
# what .github/workflows/release.yml consumes. See docs/DISTRIBUTION.md for
# the tag/channel convention and why the AppImage is only built in CI.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

die() { echo "package: $*" >&2; exit 1; }

what="${1:-}"
if [ -z "$what" ]; then
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) what=windows ;;
    Linux*)               what=linux ;;
    *) die "unknown host $(uname -s); pass 'windows' or 'linux'" ;;
  esac
fi

case "$what" in
  windows)
    command -v powershell >/dev/null 2>&1 || die "powershell not found"
    ps1="$root/scripts/package-windows.ps1"
    command -v cygpath >/dev/null 2>&1 && ps1="$(cygpath -w "$ps1")"
    exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$ps1" "${@:2}"
    ;;
  linux)
    exec "$root/scripts/package-linux.sh" "${@:2}"
    ;;
  *) die "unknown target '$what' (windows or linux)" ;;
esac
