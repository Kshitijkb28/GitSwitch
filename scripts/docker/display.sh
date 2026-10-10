#!/bin/sh
# A virtual display shown in the host's browser: Xvfb, a window manager,
# x11vnc on it (container-local only) and noVNC on port 6080. Sourced by
# app.sh and dev.sh, so the display lives as long as they do.
mkdir -p "${XDG_RUNTIME_DIR:-/tmp/xdg}" && chmod 700 "${XDG_RUNTIME_DIR:-/tmp/xdg}"
# A restarted container keeps its /tmp, and the last Xvfb was killed, not
# stopped: its lock and socket would pass for a server that is not there.
rm -f /tmp/.X99-lock /tmp/.X11-unix/X99
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
xvfb=$!
i=0
while [ ! -S /tmp/.X11-unix/X99 ] && kill -0 "$xvfb" 2>/dev/null && [ "$i" -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
[ -S /tmp/.X11-unix/X99 ] || { echo "the virtual display did not start:" >&2; cat /tmp/xvfb.log >&2; exit 1; }
openbox >/dev/null 2>&1 &
x11vnc -display :99 -localhost -forever -shared -nopw -quiet >/tmp/x11vnc.log 2>&1 &
websockify --web /usr/share/novnc 6080 localhost:5900 >/tmp/novnc.log 2>&1 &
echo "GitSwitch's screen: open http://localhost:6080/vnc.html on this computer"
