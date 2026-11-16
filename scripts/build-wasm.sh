#!/usr/bin/env bash
# Build the browser demo: metal-dash for wasm32-unknown-unknown, bound to JS with
# wasm-bindgen, into web/wasm/pkg/. Serve web/wasm/ over HTTP to look at it
# (wasm will not load from file://).
#
#   scripts/build-wasm.sh            # release build
#   scripts/build-wasm.sh --debug    # faster, larger
#
# Needs rustup (installs the wasm32 target if missing) and wasm-bindgen-cli at
# the same version as the `wasm-bindgen` the crate resolves to. The script
# installs that version when the installed one differs.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

profile=release
profile_dir=release
if [[ "${1:-}" == "--debug" ]]; then
  profile=dev
  profile_dir=debug
fi

target=wasm32-unknown-unknown
target_dir="${CARGO_TARGET_DIR:-$root/target}"

if ! rustup target list --installed | grep -qx "$target"; then
  echo "installing rust target $target"
  rustup target add "$target"
fi

# The CLI must match the library version exactly, or the glue will not load.
want="$(cargo tree -p metal-dash --target "$target" --no-default-features -e normal \
  -i wasm-bindgen --prefix none 2>/dev/null | head -1 | awk '{print $2}' | tr -d 'v')"
if [[ -z "$want" ]]; then
  echo "wasm-bindgen is not in the metal-dash wasm tree; is the wasm target dependency gone?" >&2
  exit 1
fi
have="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)"
if [[ "$have" != "$want" ]]; then
  echo "installing wasm-bindgen-cli $want (have: ${have:-none})"
  cargo install wasm-bindgen-cli --version "$want" --locked
fi

if [[ "$profile" == "dev" ]]; then
  cargo build -p metal-dash --lib --target "$target" --no-default-features
else
  cargo build -p metal-dash --lib --target "$target" --no-default-features --release
fi

wasm="$target_dir/$target/$profile_dir/metal_dash.wasm"
wasm-bindgen --target web --no-typescript --out-dir "$root/web/wasm/pkg" "$wasm"
echo "built web/wasm/pkg/ (serve web/wasm/ and open index.html)"
