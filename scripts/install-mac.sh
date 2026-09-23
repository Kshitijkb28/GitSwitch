#!/usr/bin/env bash
# Build GitSwitch and install it cleanly into /Applications.
#
# Handles the gotchas that otherwise cause "two GitSwitch apps" on the system:
#  - removes the build-folder .app so Spotlight/Launch Services don't index it
#  - ejects any DMG volume the bundler left mounted
#  - unregisters stale Launch Services entries, keeping only /Applications
#
# Usage:  ./scripts/install-mac.sh [--skip-build]
set -euo pipefail

cd "$(dirname "$0")/.."
APP_SRC="src-tauri/target/release/bundle/macos/GitSwitch.app"
APP_DST="/Applications/GitSwitch.app"
LSREG="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

if [[ "${1:-}" != "--skip-build" ]]; then
  # Only the .app is needed to install locally. Skipping the .dmg keeps this
  # fast and sidesteps bundle_dmg.sh, whose AppleScript step fails whenever a
  # stale disk image is mounted or Finder automation is unavailable.
  # The shareable .dmg comes from CI (or `npm run tauri build` without --bundles).
  echo "▸ Building release .app…"
  # A Homebrew node with missing dylibs leaves npm unusable while bun still
  # runs the same scripts; tauri.conf.json keeps npm for CI, so override the
  # before-build command here when falling back.
  if node --version >/dev/null 2>&1; then
    npm run tauri build -- --bundles app
  else
    bun run tauri build --bundles app \
      --config '{"build":{"beforeBuildCommand":"bun run build && bash scripts/build-lock-helper.sh release"}}'
  fi
fi

[[ -d "$APP_SRC" ]] || { echo "✗ Build output not found at $APP_SRC"; exit 1; }

echo "▸ Stopping any running GitSwitch…"
osascript -e 'quit app "GitSwitch"' 2>/dev/null || true
pkill -f "GitSwitch.app/Contents/MacOS/gitswitch" 2>/dev/null || true
sleep 1

echo "▸ Installing to /Applications…"
rm -rf "$APP_DST"
cp -R "$APP_SRC" "$APP_DST"
codesign --force --deep --sign - "$APP_DST" 2>/dev/null || true

echo "▸ Cleaning up build copy + stray DMG mounts…"
rm -rf "$APP_SRC"
for v in /Volumes/dmg.*; do
  [[ -e "$v" ]] && hdiutil detach "$v" -force >/dev/null 2>&1 || true
done

echo "▸ Fixing Launch Services registrations…"
"$LSREG" -dump 2>/dev/null \
  | grep -oE '/[^ ]*GitSwitch.app' | sort -u \
  | grep -v "^${APP_DST}$" \
  | while read -r p; do "$LSREG" -u "$p" 2>/dev/null || true; done
"$LSREG" -f "$APP_DST" 2>/dev/null || true

echo "▸ Launching…"
open "$APP_DST"
echo "✓ Installed and launched $APP_DST"
echo "  For a shareable .dmg use CI, or: npm run tauri build   (bun run tauri build when node is broken)"
