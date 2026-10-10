#!/bin/sh
# `tauri dev` on the checkout mounted at /app (the Dockerfile's `dev` target).
set -eu
. /usr/local/bin/display.sh
cd /app
# node_modules is a volume of its own: Linux packages, never the host's. It
# outlives the container, so it is installed again whenever the lockfile has
# changed since (a pull, another branch).
want="$(sha256sum package-lock.json | cut -d' ' -f1)"
if [ ! -x node_modules/.bin/tauri ] || [ "$(cat node_modules/.lock-sha256 2>/dev/null)" != "$want" ]; then
  npm ci
  echo "$want" > node_modules/.lock-sha256
fi
exec dbus-run-session -- npm run tauri dev
