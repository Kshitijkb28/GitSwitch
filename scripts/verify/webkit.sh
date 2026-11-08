#!/bin/bash
# The terminal drawer in the system's WebKit — the engine the app itself runs
# in on macOS (a Tauri window is a WKWebView). harness/ui.mjs proves the same
# things in Chrome; a layout that holds in one engine can fail in the other,
# and "the last lines of the terminal cannot be reached" is exactly the kind
# of fault that only shows where the app really runs.
#
# What is measured, inside the page: the rows the terminal draws, the box they
# are drawn in, and the rows the shell was last told it has. All three must
# agree — when the shell believes in more rows than are drawn, a full-screen
# program paints its last lines below the visible edge until something
# resizes the drawer.
#
# No git and no backend here: the page is the built frontend with the browser
# harness's own mock bridge, handed to WebKit by harness/webkit-runner.swift.
# A real window appears for a few seconds per run (a hidden one is throttled
# and would prove nothing); it never takes the keyboard and closes by itself.
set -u
source "$(dirname "$0")/lib.sh"
VERIFY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$VERIFY_DIR/../.." && pwd)"
H="$VERIFY_DIR/harness"
tmp="${TMPDIR:-${TEMP:-/tmp}}"; tmp="${tmp%/}"
WORK="${GITSWITCH_VERIFY_TMP:-$tmp/gitswitch-verify}"
SB="$WORK/webkit"
REAL_HOME="$HOME"                 # bun and swiftc keep their caches there
rm -rf "$SB"; mkdir -p "$SB/home"
export HOME="$SB/home"            # FIRST, as in verify_init — nothing below writes a config, and nothing may
PASS=0; FAIL=0

skip() { printf 'SKIPPED: %s\n' "$1"; exit 0; }
[ "$(uname -s)" = Darwin ] || skip "the system WebKit is the app's engine on macOS only (Linux uses WebKitGTK, Windows WebView2)."
command -v swiftc >/dev/null 2>&1 || skip "swiftc is not installed (xcode-select --install); the Chrome harness still covers the same checks."
[ -z "${CI:-}" ] || skip "needs a window on a real screen; run it on a Mac."
if HOME="$REAL_HOME" node --version >/dev/null 2>&1; then JS=node; elif command -v bun >/dev/null 2>&1; then JS=bun; else skip "neither node nor bun works."; fi
[ -f "$ROOT/dist/index.html" ] || { echo "dist/ is missing — build the frontend first (npm run build / bun run build)"; exit 1; }

# The runner is compiled once per version of its source.
RUNNER="$WORK/.webkit-runner-$(sha256file "$H/webkit-runner.swift" | cut -c1-12)"
if [ ! -x "$RUNNER" ]; then
  HOME="$REAL_HOME" swiftc -O -swift-version 5 "$H/webkit-runner.swift" -o "$RUNNER" 2>"$SB/swiftc.log" \
    || { echo "could not compile harness/webkit-runner.swift:"; tail -n 20 "$SB/swiftc.log"; exit 1; }
fi

( cd "$H" && exec env HOME="$REAL_HOME" "$JS" serve.mjs ) > "$SB/serve.out" 2>&1 &
SERVE_PID=$!
trap 'kill "$SERVE_PID" 2>/dev/null' EXIT
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do [ -s "$SB/serve.out" ] && break; sleep 0.5; done
URL="$(head -1 "$SB/serve.out")"
case "$URL" in http://127.0.0.1:*) ;; *) echo "the page server did not start: $(cat "$SB/serve.out")"; exit 1 ;; esac

# measure <label> <ms the shell takes to start> [window width] -> $SB/<label>.json
measure() {
  ( cd "$H" && HOME="$REAL_HOME" GITSWITCH_BRIDGE_OUT="$SB/$1.bridge.js" GITSWITCH_TERMINAL_DELAY_MS="$2" "$JS" ui.mjs ) \
    || { echo "could not write the bridge for $1"; exit 1; }
  "$RUNNER" "$URL/repo/changes" "$SB/$1.bridge.js" "$H/webkit-steps.js" "${3:-1280}" 900 > "$SB/$1.json" 2>"$SB/$1.err" || true
}
# field <label> <python expression over d> -> its value
field() { "$PY" -c "
import sys,json
try: d=json.load(open(sys.argv[1]))
except Exception as e: print('unreadable: %s' % e); sys.exit(0)
fits=lambda s: bool(s) and s['rows']>0 and s['lastBottom']<=s['boxBottom']+1 and s['lastBottom']<=s['windowHeight'] and s['boxHeight']-s['rows']*s['cell']<s['cell']
try: print($2)
except Exception as e: print('missing: %s — %s' % (e, json.dumps(d)[:300]))
" "$SB/$1.json"; }

check() {
  local label="$1" delay="$2" title="$3"
  measure "$label" "$delay" "${4:-1280}"
  section "$title"
  ok "the app loads in WebKit and the terminal opens" "$(field "$label" "'%s %s %s' % (d.get('loaded'), d.get('opened'), 'AppleWebKit' in d.get('engine',''))")" "True True True"
  ok "  every row drawn is inside the drawer, and the box is used: one more row would not fit" "$(field "$label" "fits(d['early'])")" "True"
  ok "when the box shrinks (the note opens above it) the rows are fitted again" "$(field "$label" "d['noteOpen']['rows'] < d['early']['rows'] and fits(d['noteOpen'])")" "True"
  ok "  and the shell is told the rows that are drawn" "$(field "$label" "'%s=%s by %s' % (d['noteOpen']['toldRows'], d['noteOpen']['rows'], d['noteOpen']['toldBy'])" | "$PY" -c "import sys; a=sys.stdin.read().strip(); l,r=a.split(' by ')[0].split('='); print('agree' if l==r and a.endswith('terminal_resize') else 'differ: '+a)")" "agree"
  ok "when the box grows back the rows return, still inside it" "$(field "$label" "d['noteClosed']['rows'] == d['early']['rows'] and fits(d['noteClosed'])")" "True"
  ok "  and the shell is told again" "$(field "$label" "d['noteClosed']['toldRows'] == d['noteClosed']['rows']")" "True"
}

check at-once 0 "The terminal in the app's own engine, with a shell that starts at once"
# The box changes shape before the session exists: the resize has nowhere to
# go at that moment, and used to be lost for good.
check late 1800 "The same, with a shell that takes 1.8 s to start"
# Half a screen wide: the drawer's own header wraps onto two lines there,
# which takes the height from the terminal below it.
check narrow 1800 "The same at half-screen width (700 px), where the drawer's header wraps" 700

printf '\nengine: %s\n' "$(field at-once "d.get('engine','?')")"
verify_result
