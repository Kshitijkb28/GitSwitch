#!/bin/bash
# The Uninstall tab, end to end against the real backend in a sandbox.
#
# Stand-in apps under a re-rooted /Applications (macOS) or a stand-in
# AppImage with its menu entry (Linux); their leftovers in the sandbox home
# beside folders that only look like theirs: a sibling product's, a shared
# vendor folder's other app, generic names, a link to a precious folder.
# Uninstalling moves the app to a sandbox "Trash"; the scan refuses while the
# app is installed and then offers exactly its own leftovers, sure ones ticked;
# deleting moves only what was ticked, and refuses an item swapped or reached
# through a link since the scan, and everything once the app is back.
set -u
source "$(dirname "$0")/lib.sh"
verify_init apps "Apps Tester" "apps@example.test"
SB="$(realdir "$SB")"
if command -v cygpath >/dev/null 2>&1; then
  echo "apps suite: skipped on Windows (the profile store does not follow HOME there; the Windows rules are unit tests in apps/windows.rs)"
  verify_result
  exit $?
fi
unset XDG_DATA_HOME XDG_CONFIG_HOME XDG_CACHE_HOME
H="$(realdir "$HOME")"; export HOME="$H"
ROOT="$SB/root"; TRASH="$SB/trash"
mkdir -p "$ROOT" "$TRASH"
export GITSWITCH_APPS_ROOT="$ROOT" GITSWITCH_TRASH_DIR="$TRASH"

# steps <json list of {cmd, args}> -> the list of results, one per step
steps() { pinvoke apps_script "$("$PY" -c 'import json,sys; print(json.dumps({"steps": json.loads(sys.argv[1])}))' "$1")"; }
# pick <python expression over r (the step results)>
pick() { "$PY" -c "import sys,json; d=json.load(sys.stdin); r=d.get('value',d); print(eval(sys.argv[1]))" "$1"; }
there() { [ -e "$1" ] || [ -L "$1" ] && echo there || echo gone; }

if [ "$(uname -s)" = Darwin ]; then
  plist() { # plist <bundle> <id> <name> <exe>
    mkdir -p "$1/Contents/MacOS"
    cat > "$1/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$2</string><key>CFBundleName</key><string>$3</string>
<key>CFBundleExecutable</key><string>$4</string><key>CFBundleShortVersionString</key><string>1.0</string>
</dict></plist>
EOF
  }
  APP="$ROOT/Applications/Foo Writer.app"
  make_apps() {
    plist "$APP" com.acme.foowriter "Foo Writer" FooWriter
    plist "$APP/Contents/Library/LoginItems/Foo Helper.app" com.acme.foowriter.helper "Foo Helper" FooHelper
  }
  make_apps
  plist "$ROOT/Applications/Foo Writer Beta.app" com.acme.foowriter.beta "Foo Writer Beta" FooWriterBeta
  plist "$ROOT/Applications/Bar Tool.app" com.acme.bartool "Bar Tool" BarTool
  L="$H/Library"
  make_leftovers() {
    mkdir -p "$L/Application Support/Foo Writer" "$L/Application Support/com.acme.foowriter" "$L/Application Support/com.acme.foowriter.beta" \
      "$L/Application Support/acme/Foo Writer" "$L/Application Support/acme/Bar Tool" "$L/Application Support/FooWriterBackup" "$L/Application Support/Electron" \
      "$L/Caches/com.acme.foowriter" "$L/Caches/com.acme.foowriter.ShipIt" "$L/Caches/com.acme.foowriter.helper" "$L/Containers/com.acme.foowriter" \
      "$L/Logs/Foo Writer" "$L/Preferences" "$L/LaunchAgents" "$SB/precious" "$ROOT/Library/Application Support/com.acme.foowriter" "$ROOT/usr/local/bin"
    printf 'mine' > "$SB/precious/keep.txt"
    ln -sfn "$SB/precious" "$L/Application Support/FooWriter"
    printf 'x' > "$L/Application Support/Foo Writer/settings.json"
    printf '%s' "$(head -c 3000 /dev/zero | tr '\0' x)" > "$L/Caches/com.acme.foowriter/blob"
    for f in com.acme.foowriter.plist com.acme.foowriter.beta.plist com.acme.foowriter.extra.plist; do printf 'p' > "$L/Preferences/$f"; done
    cat > "$L/LaunchAgents/com.acme.foowriter.agent.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>Label</key><string>com.acme.foowriter.agent</string>
<key>ProgramArguments</key><array><string>$APP/Contents/MacOS/agent</string></array></dict></plist>
EOF
    cat > "$L/LaunchAgents/com.other.agent.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>Label</key><string>com.other.agent</string>
<key>Program</key><string>/usr/bin/true</string></dict></plist>
EOF
    ln -sfn "$APP/Contents/MacOS/cli" "$ROOT/usr/local/bin/foowriter"
  }
  make_leftovers
  ID="mac:$APP"

  section "The list, and what a removal would do"
  OUT=$(steps '[{"cmd":"list","args":{"kind":"apps"}}]')
  ok "the stand-in apps are listed, nothing else" "$(printf '%s' "$OUT" | pick "sorted(a['name'] for a in r[0]['value']['apps'])")" "['Bar Tool', 'Foo Writer', 'Foo Writer Beta']"
  OUT=$(steps "$("$PY" -c 'import json,sys; print(json.dumps([{"cmd":"prepare","args":{"id":sys.argv[1]}},{"cmd":"scan","args":{"session":"$session"}}]))' "$ID")")
  ok "it will go to the Trash" "$(printf '%s' "$OUT" | pick "r[0]['value']['method']")" "is moved to the Trash"
  ok "  what identifies it: its id and the id inside it" "$(printf '%s' "$OUT" | pick "' | '.join(r[0]['value']['facts'])")" "Inside it: com.acme.foowriter.helper"
  ok "no leftovers are looked for while it is installed" "$(printf '%s' "$OUT" | pick "(r[1]['ok'], 'still installed' in r[1]['error'])")" "(False, True)"

  section "Uninstalled: exactly its own leftovers, the sure ones ticked"
  OUT=$(steps "$("$PY" -c 'import json,sys; print(json.dumps([{"cmd":"prepare","args":{"id":sys.argv[1]}},{"cmd":"uninstall","args":{"session":"$session"}},{"cmd":"scan","args":{"session":"$session"}}]))' "$ID")")
  ok "it is gone, into the Trash" "$(printf '%s' "$OUT" | pick "r[1]['value']['state']") $(there "$APP") $(there "$TRASH/Foo Writer.app")" "gone gone there"
  rel() { printf '%s' "$OUT" | "$PY" -c "import sys,json; d=json.load(sys.stdin); r=d['value']; h=sys.argv[1]; rt=sys.argv[2]; print(sorted((i['path'].replace(h,'~').replace(rt,'ROOT'), i['confidence'], i['admin']) for i in r[2]['value']['items']))" "$H" "$ROOT"; }
  GOT="$(rel)"
  want_in() { ok "  offered: $1 ($2)" "$GOT" "$1', '$2', $3)"; }
  want_in "~/Library/Application Support/com.acme.foowriter" high False
  want_in "~/Library/Application Support/Foo Writer" medium False
  want_in "~/Library/Application Support/acme/Foo Writer" medium False
  want_in "~/Library/Caches/com.acme.foowriter" high False
  want_in "~/Library/Caches/com.acme.foowriter.ShipIt" high False
  want_in "~/Library/Caches/com.acme.foowriter.helper" high False
  want_in "~/Library/Containers/com.acme.foowriter" high False
  want_in "~/Library/Logs/Foo Writer" medium False
  want_in "~/Library/Preferences/com.acme.foowriter.plist" high False
  want_in "~/Library/Preferences/com.acme.foowriter.extra.plist" medium False
  want_in "~/Library/LaunchAgents/com.acme.foowriter.agent.plist" high False
  want_in "ROOT/usr/local/bin/foowriter" high False
  want_in "ROOT/Library/Application Support/com.acme.foowriter" high True
  ok "  and nothing else (13 items)" "$(printf '%s' "$OUT" | pick "len(r[2]['value']['items'])")" "13"
  for never in "com.acme.foowriter.beta'" "Bar Tool" "FooWriterBackup" "Electron" "Support/FooWriter'" "com.other.agent" "acme'"; do
    ok "  never offered: ${never%\'}" "$(printf '%s' "$GOT" | grep -c -- "$never" || true)" "0"
  done
  ok "the administrator's one comes with a command, not a checkbox" "$(printf '%s' "$OUT" | pick "[i['command'] for i in r[2]['value']['items'] if i['admin']][0]")" "sudo rm -rf"
  ok "sizes are counted (not inside Containers)" "$(printf '%s' "$OUT" | pick "[(i['path'].rsplit('/',1)[-1], i['size']) for i in r[2]['value']['items'] if i['path'].endswith('Caches/com.acme.foowriter') or 'Containers' in i['path']]")" "('com.acme.foowriter', 3000), ('com.acme.foowriter', None)"

  section "Deleting the ticked ones: to the Trash, nothing else touched"
  OUT=$(steps '[{"cmd":"sessions","args":{}}]')
  SESSION=$(printf '%s' "$OUT" | pick "r[0]['value'][0]['session']")
  OUT=$(steps "$("$PY" -c 'import json,sys; print(json.dumps([{"cmd":"scan","args":{"session":sys.argv[1]}},{"cmd":"delete","args":{"scan":"$scan","ids":"$ids:high"}}]))' "$SESSION")")
  ok "a scan after a restart still knows the app (the session is on disk)" "$(printf '%s' "$OUT" | pick "r[0]['ok']")" "True"
  ok "the sure ones (not the administrator's) are moved" "$(printf '%s' "$OUT" | pick "sorted(set(x['outcome'] for x in r[1]['value']))")" "['trashed']"
  ok "  8 of them" "$(printf '%s' "$OUT" | pick "len(r[1]['value'])")" "8"
  ok "  into the Trash" "$(there "$TRASH/com.acme.foowriter.plist") $(there "$TRASH/foowriter") $(there "$L/Caches/com.acme.foowriter")" "there there gone"
  ok "the maybes stay until ticked" "$(there "$L/Application Support/Foo Writer") $(there "$L/Logs/Foo Writer")" "there there"
  ok "the sibling's, the other app's, the generic and the link's target are untouched" "$(there "$L/Application Support/com.acme.foowriter.beta") $(there "$L/Preferences/com.acme.foowriter.beta.plist") $(there "$L/Application Support/acme/Bar Tool") $(there "$L/Application Support/Electron") $(cat "$SB/precious/keep.txt")" "there there there there mine"
  ok "the administrator's one is not deleted" "$(there "$ROOT/Library/Application Support/com.acme.foowriter")" "there"
  ok "the agent of another app is untouched" "$(there "$L/LaunchAgents/com.other.agent.plist")" "there"

  section "Checked again before deleting"
  MUTATE='rm -rf "$LIB/Logs/Foo Writer" && mkdir "$LIB/Logs/Foo Writer" && mv "$LIB/Application Support/acme" "$LIB/acme-real" && ln -s "$LIB/acme-real" "$LIB/Application Support/acme"'
  STEPS=$("$PY" - "$SESSION" "$L" "$MUTATE" <<'EOF'
import json, sys
s, lib, mutate = sys.argv[1], sys.argv[2], sys.argv[3]
print(json.dumps([
    {"cmd": "scan", "args": {"session": s}},
    {"cmd": "sh", "args": {"run": "LIB='%s'; %s" % (lib, mutate)}},
    {"cmd": "delete", "args": {"scan": "$scan", "ids": "$ids:all"}},
]))
EOF
)
  OUT=$(steps "$STEPS")
  RES=$(printf '%s' "$OUT" | "$PY" -c "import sys,json; r=json.load(sys.stdin)['value']; items={i['id']:i['path'] for i in r[0]['value']['items']}; print(sorted((items[x['id']].rsplit('/',2)[-2]+'/'+items[x['id']].rsplit('/',1)[-1], x['outcome']) for x in r[2]['value']))")
  ok "a folder swapped for another with the same name is kept" "$RES" "('Logs/Foo Writer', 'changed')"
  ok "an item now reached through a link is kept" "$RES" "('acme/Foo Writer', 'refused')"
  ok "  and what is behind the link is untouched" "$(there "$L/acme-real/Foo Writer")" "there"
  ok "the administrator's one is still not deleted" "$RES" "Support/com.acme.foowriter', 'needs-admin')"
  ok "an unticked-before maybe, ticked now, goes" "$RES" "('Application Support/Foo Writer', 'trashed')"
  make_apps; make_leftovers
  OUT=$(steps "$("$PY" -c 'import json,sys; print(json.dumps([{"cmd":"scan","args":{"session":sys.argv[1]}}]))' "$SESSION")")
  ok "reinstalled: no scan" "$(printf '%s' "$OUT" | pick "(r[0]['ok'], 'still installed' in r[0].get('error',''))")" "(False, True)"
fi

if [ "$(uname -s)" = Linux ]; then
  mkdir -p "$ROOT/usr/share/applications" "$H/Apps" "$H/.local/share/applications" "$H/.config/autostart" "$H/.config/Foo Image" "$H/.config/FooImage" "$H/.cache/FooImage" "$H/.config/FooImageOld" "$H/.config/gtk-3.0" "$SB/precious"
  IMG="$H/Apps/FooImage.AppImage"
  printf '\177ELF\002\001\001\000AI\002' > "$IMG"; head -c 64 /dev/zero >> "$IMG"; chmod +x "$IMG"
  printf '[Desktop Entry]\nName=Foo Image\nExec="%s" %%U\nType=Application\n' "$IMG" > "$H/.local/share/applications/fooimage.desktop"
  cp "$H/.local/share/applications/fooimage.desktop" "$H/.config/autostart/fooimage.desktop"
  printf '[Desktop Entry]\nName=My Web App\nExec=/opt/google/chrome/google-chrome --app-id=abc\nType=Application\n' > "$H/.local/share/applications/chrome-abc-Default.desktop"
  ln -sfn "$SB/precious" "$H/.config/Foo-Image"
  ID="appimage:$IMG"

  section "An AppImage: listed by its menu entry, gone with it"
  OUT=$(steps '[{"cmd":"list","args":{"kind":"apps"}}]')
  ok "the AppImage is listed as one" "$(printf '%s' "$OUT" | pick "[(a['name'], a['source']) for a in r[0]['value']['apps'] if a['source']=='appimage']")" "[('Foo Image', 'appimage')]"
  ok "a browser's web app is never taken for the browser" "$(printf '%s' "$OUT" | pick "[a['name'] for a in r[0]['value']['apps'] if 'Web App' in a['name'] or 'chrome' in a['name'].lower()]")" "[]"
  ok "  and it is said why it is not listed" "$(printf '%s' "$OUT" | pick "r[0]['value']['notes']")" "belong to no package"
  OUT=$(steps "$("$PY" -c 'import json,sys; print(json.dumps([{"cmd":"prepare","args":{"id":sys.argv[1]}},{"cmd":"uninstall","args":{"session":"$session"}},{"cmd":"scan","args":{"session":"$session"}},{"cmd":"delete","args":{"scan":"$scan","ids":"$ids:all"}}]))' "$ID")")
  ok "uninstalled: the file and its menu entry to the Trash" "$(printf '%s' "$OUT" | pick "r[1]['value']['state']") $(there "$IMG") $(there "$TRASH/FooImage.AppImage") $(there "$H/.local/share/applications/fooimage.desktop")" "gone gone there gone"
  ok "its leftovers are maybes only, by exact name" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json; r=json.load(sys.stdin)['value']; print(sorted((i['path'].replace(sys.argv[1],'~'), i['confidence']) for i in r[2]['value']['items']))" "$H")" "[('~/.cache/FooImage', 'medium'), ('~/.config/Foo Image', 'medium'), ('~/.config/FooImage', 'medium'), ('~/.config/autostart/fooimage.desktop', 'medium')]"
  ok "deleting them moves them" "$(printf '%s' "$OUT" | pick "sorted(set(x['outcome'] for x in r[3]['value']))")" "['trashed']"
  ok "the look-alikes and the link's target are untouched" "$(there "$H/.config/FooImageOld") $(there "$H/.config/gtk-3.0") $(there "$H/.config/Foo-Image") $(there "$SB/precious")" "there there there there"
fi

# APPS_REAL_PACKAGES=1, as root, on a throwaway Debian/Ubuntu system (a
# container): the real dpkg removes a real package (xterm), and purges what
# it left in /etc. Do not run it on a machine you care about.
if [ "$(uname -s)" = Linux ] && [ "${APPS_REAL_PACKAGES:-}" = 1 ] && [ "$(id -u)" = 0 ]; then
  unset GITSWITCH_APPS_ROOT
  dpkg-query -W -f '${db:Status-Abbrev}' xterm 2>/dev/null | grep -q '^ii' || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq xterm >/dev/null 2>&1
  mkdir -p "$H/.config/xterm" "$H/.config/xtermish"

  section "A real package: apt says what goes with it, dpkg removes exactly that, then its settings"
  OUT=$(steps '[{"cmd":"list","args":{"kind":"apps"}},{"cmd":"list","args":{"kind":"packages"}}]')
  ok "xterm is listed as the package's app" "$(printf '%s' "$OUT" | pick "[(a['id'], a['source']) for a in r[0]['value']['apps'] if a['id']=='deb:xterm']")" "[('deb:xterm', 'deb')]"
  ok "the system's own packages can't be removed" "$(printf '%s' "$OUT" | pick "sorted(a['name'] for a in r[1]['value']['apps'] if a['name'] in ('dpkg','apt','bash','libc6') and a['blocked'])")" "['apt', 'bash', 'dpkg', 'libc6']"
  OUT=$(steps '[{"cmd":"prepare","args":{"id":"deb:xterm"}},{"cmd":"uninstall","args":{"session":"$session"}},{"cmd":"scan","args":{"session":"$session"}},{"cmd":"delete","args":{"scan":"$scan","ids":"$ids:all"}}]')
  ok "the command that runs is dpkg -r with apt's list" "$(printf '%s' "$OUT" | pick "r[0]['value']['command']")" "dpkg -r xterm"
  ok "it is removed" "$(printf '%s' "$OUT" | pick "r[1]['value']['state']") $(dpkg-query -W -f '${db:Status-Abbrev}' xterm 2>/dev/null | grep -q '^ii' && echo installed || echo removed)" "gone removed"
  ok "its leftovers: its settings kept by the package, and a maybe by exact name" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json; r=json.load(sys.stdin)['value']; print(sorted((i['kind'], i['confidence']) for i in r[2]['value']['items']))")" "[('folder', 'medium'), ('package-settings', 'medium')]"
  ok "deleting: the folder to the Trash, the settings purged" "$(printf '%s' "$OUT" | pick "sorted(x['outcome'] for x in r[3]['value'])") $(dpkg-query -W -f '${db:Status-Abbrev}' xterm 2>/dev/null || echo none)" "['removed', 'trashed']"
  ok "  nothing of it is left in dpkg (not even its settings)" "$(dpkg-query -W -f '${db:Status-Abbrev}' xterm 2>/dev/null | grep -Eq '^(rc|ii)' && echo still-there || echo purged)" "purged"
  ok "the look-alike stays" "$(there "$H/.config/xtermish")" "there"
fi

verify_result
