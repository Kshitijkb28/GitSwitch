#!/bin/bash
# Build the sidecars (the lock helper and the terminal's git guard) where
# Tauri's bundler — and its build script — expect them:
#   src-tauri/binaries/<name>-<target-triple>[.exe]
# The work is done by src-tauri/sidecars (a small Rust program), which Tauri's
# before-commands run directly, so no shell is needed on any OS; this wrapper
# keeps the scripts and CI steps that call it working.
# Usage: scripts/build-lock-helper.sh [release|debug]
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec cargo run --quiet --manifest-path "$ROOT/src-tauri/Cargo.toml" -p gitswitch-sidecars -- "$@"
