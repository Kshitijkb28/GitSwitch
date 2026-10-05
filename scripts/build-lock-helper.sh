#!/bin/bash
# Build the sidecars (the lock helper and the terminal's git guard) and place
# them where Tauri's bundler — and its build script, which refuses to compile
# the app without them — expect them:
#   src-tauri/binaries/<name>-<target-triple>[.exe]
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

build_one() { # <package> <triple> -> echoes the built binary's path
  local pkg="$1" t="$2"
  local flags=()
  [ "$PROFILE" = release ] && flags+=(--release)
  if [ "$t" = "$HOST" ]; then
    (cd "$SRC" && cargo build -p "$pkg" ${flags[@]+"${flags[@]}"} >&2)
    echo "$SRC/target/$PROFILE/$pkg$EXT"
  else
    (cd "$SRC" && cargo build -p "$pkg" --target "$t" ${flags[@]+"${flags[@]}"} >&2)
    echo "$SRC/target/$t/$PROFILE/$pkg$EXT"
  fi
}

# Two sidecars: the privileged lock helper, and the terminal's git guard
# (used on Windows, where there is no sh for the script form).
for PKG in gitswitch-lock-helper gitswitch-git-shim; do
  OUT="$SRC/binaries/$PKG-$TRIPLE$EXT"
  if [ "$TRIPLE" = universal-apple-darwin ]; then
    A="$(build_one "$PKG" aarch64-apple-darwin)"
    X="$(build_one "$PKG" x86_64-apple-darwin)"
    lipo -create "$A" "$X" -output "$OUT"
  else
    cp "$(build_one "$PKG" "$TRIPLE")" "$OUT"
  fi
  chmod 755 "$OUT"
  echo "sidecar: $OUT"
done
