#!/bin/bash
# Inside the Linux test image (scripts/docker/verify.sh): the real app on a
# virtual display, to see what a start at sign-in does with its window.
#   bash scripts/docker/verify.sh bash scripts/docker/startup-check.sh
# Started with --startup and the speed shown: no window, and the autostart
# entry kept; a second sign-in start changes nothing; GitSwitch opened again
# by hand shows the window; a plain start shows it at once.
set -u
cd "${GITSWITCH_SRC:-$(dirname "$0")/../..}/src-tauri" || exit 1
cargo build -q -p gitswitch --bin gitswitch || exit 1
export APP="$PWD/target/debug/gitswitch"

# A throwaway home, FIRST: the app writes its settings and git config there.
export HOME="$(mktemp -d)"
export GITSWITCH_AUTOSTART_DIR="$HOME/autostart" XDG_RUNTIME_DIR="$HOME/xdg" DISPLAY=:99
mkdir -p "$XDG_RUNTIME_DIR" "$HOME/.local/share/com.gitswitch.app" && chmod 700 "$XDG_RUNTIME_DIR"
printf '{"tray":true,"start_at_login":true}' > "$HOME/.local/share/com.gitswitch.app/netspeed.json"
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
XVFB=$!
for _ in $(seq 50); do [ -S /tmp/.X11-unix/X99 ] && break; sleep 0.1; done

# The session bus GitSwitch's single-instance check and tray icon use.
dbus-run-session -- bash -c '
  PASS=0; FAIL=0
  ok() { if [ "$2" = "$3" ]; then PASS=$((PASS+1)); echo "  PASS $1"; else FAIL=$((FAIL+1)); echo "  FAIL $1 (got $2, want $3)"; fi; }
  shown() { xdotool search --onlyvisible --name "^GitSwitch$" 2>/dev/null | wc -l | tr -d " "; }
  until_shown() { for _ in $(seq "$1"); do [ "$(shown)" -gt 0 ] && return; sleep 1; done; }
  "$APP" --startup >"$HOME/app1.log" 2>&1 &
  P=$!
  sleep 25
  ok "started at sign-in: running" "$(kill -0 $P 2>/dev/null && echo yes)" "yes"
  ok "  with no window on screen" "$(shown)" "0"
  ok "  its autostart entry kept, naming this program" "$(grep -c "^Exec=$APP --startup$" "$GITSWITCH_AUTOSTART_DIR/gitswitch.desktop" 2>/dev/null)" "1"
  "$APP" --startup >/dev/null 2>&1
  sleep 3
  ok "a second sign-in start changes nothing" "$(shown)" "0"
  "$APP" >/dev/null 2>&1
  until_shown 15
  ok "opened again by hand: the window shows" "$([ "$(shown)" -gt 0 ] && echo yes)" "yes"
  kill $P; wait $P 2>/dev/null
  "$APP" >"$HOME/app2.log" 2>&1 &
  P=$!
  until_shown 25
  ok "a plain start shows the window" "$([ "$(shown)" -gt 0 ] && echo yes)" "yes"
  kill $P; wait $P 2>/dev/null
  echo "RESULT  $PASS passed, $FAIL failed"
  [ $FAIL -eq 0 ] || { echo "--- the sign-in start"; tail -20 "$HOME/app1.log"; echo "--- the plain start"; tail -20 "$HOME/app2.log"; exit 1; }
'
rc=$?
kill "$XVFB" 2>/dev/null
exit $rc
