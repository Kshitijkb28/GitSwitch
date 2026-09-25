#!/bin/bash
# Build the lock-helper sidecar and place it where Tauri's bundler — and its
# build script, which refuses to compile the app without it — expect it:
#   src-tauri/binaries/gitswitch-lock-helper-<target-triple>[.exe]
# Usage: scripts/build-lock-helper.sh [release|debug]
# Tauri sets TAURI_ENV_TARGET_TRIPLE for before-commands; `universal-apple-darwin`
# is built for both Apple architectures and joined with lipo.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
SRC="$ROOT/src-tauri"
PROFILE="${1:-release}"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
TRIPLE="${TAURI_ENV_TARGET_TRIPLE:-$HOST}"
EXT=""; case "$TRIPLE" in *windows*) EXT=".exe";; esac
mkdir -p "$SRC/binaries"

build_one() { # <triple> -> echoes the built binary's path
  local t="$1"
  local flags=()
  [ "$PROFILE" = release ] && flags+=(--release)
  if [ "$t" = "$HOST" ]; then
    (cd "$SRC" && cargo build -p gitswitch-lock-helper ${flags[@]+"${flags[@]}"} >&2)
    echo "$SRC/target/$PROFILE/gitswitch-lock-helper$EXT"
  else
    (cd "$SRC" && cargo build -p gitswitch-lock-helper --target "$t" ${flags[@]+"${flags[@]}"} >&2)
    echo "$SRC/target/$t/$PROFILE/gitswitch-lock-helper$EXT"
  fi
}

OUT="$SRC/binaries/gitswitch-lock-helper-$TRIPLE$EXT"
if [ "$TRIPLE" = universal-apple-darwin ]; then
  A="$(build_one aarch64-apple-darwin)"
  X="$(build_one x86_64-apple-darwin)"
  lipo -create "$A" "$X" -output "$OUT"
else
  cp "$(build_one "$TRIPLE")" "$OUT"
fi
chmod 755 "$OUT"
echo "sidecar: $OUT"
