#!/bin/sh
# The built app on the virtual display (the Dockerfile's `app` target).
set -eu
. /usr/local/bin/display.sh
# The main binary the package installed: its own entry in /usr/bin, not one
# of the two sidecars beside it.
pkg="$(dpkg-query -W -f '${Package}\n' | grep -i 'switch' | head -n 1)"
bin="$(dpkg -L "$pkg" | grep '^/usr/bin/' | grep -v -e lock-helper -e git-shim | head -n 1)"
[ -n "$bin" ] || { echo "the GitSwitch package installed no program in /usr/bin" >&2; exit 1; }
exec dbus-run-session -- "$bin"
