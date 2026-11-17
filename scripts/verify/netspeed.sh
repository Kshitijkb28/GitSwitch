#!/bin/bash
# Net speed, end to end against the real backend in a sandbox.
#
# Starting at sign-in: the entry each system keeps (Windows' Run key, a macOS
# LaunchAgent, a Linux autostart file) is made, kept, corrected and removed
# with the setting, and only while the speed is shown. Windows' Startup apps
# turning it off is reported and left alone. A start at sign-in keeps the
# window hidden only with something to show. A test build without the
# sandbox's override has nowhere to write.
#
# With NETSPEED_LIVE=1, also the speed itself during a real download (50 MB
# from Cloudflare's speed test, the endpoint the app's own test uses), held
# against curl's own measure and the system's own byte counter.
set -u
source "$(dirname "$0")/lib.sh"
verify_init netspeed "Net Tester" "net@example.test"      # HOME redirected FIRST
SB="$(realdir "$SB")"
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) PLAT=windows ;; Darwin) PLAT=macos ;; *) PLAT=linux ;; esac
if [ "$PLAT" = windows ]; then LOC="$GITSWITCH_AUTOSTART_KEY"; else LOC="$GITSWITCH_AUTOSTART_DIR"; fi

v() { "$PY" -c "import sys,json; v=json.load(sys.stdin)['value']; print($1)"; }
save() { pinvoke netspeed_set_settings "{\"settings\":$1}"; }
snap() { pinvoke netspeed_snapshot '{}'; }

# The entry as the system keeps it, read back with the system's own formats:
# {exists, ok (starts this program with --startup), why, stamp (changes on a write)}.
entry() {
  "$PY" - "$PLAT" "$LOC" "$BIN" "${1:-read}" <<'EOF'
import sys, os, json, shlex
plat, loc, binpath, what = sys.argv[1:5]
out = {"exists": False, "ok": False, "why": "", "stamp": None}

def same(p):
    try:
        return os.path.samefile(p, binpath)
    except OSError:
        return False

if plat == "windows":
    import winreg
    H = winreg.HKEY_CURRENT_USER
    def rmtree(path):
        try:
            k = winreg.OpenKey(H, path, 0, winreg.KEY_ALL_ACCESS)
        except OSError:
            return
        while True:
            try:
                sub = winreg.EnumKey(k, 0)
            except OSError:
                break
            rmtree(path + "\\" + sub)
        winreg.CloseKey(k)
        winreg.DeleteKey(H, path)
    if what == "clean":
        rmtree(loc)
        try:
            winreg.DeleteKey(H, loc.rsplit("\\", 1)[0])   # the suites' parent key, once empty
        except OSError:
            pass
        print(json.dumps(out)); sys.exit()
    if what.startswith("approve="):
        k = winreg.CreateKey(H, loc + r"\StartupApproved\Run")
        winreg.SetValueEx(k, "GitSwitch", 0, winreg.REG_BINARY, bytes([int(what[8:], 16)] + [0] * 11))
        print(json.dumps(out)); sys.exit()
    if what.startswith("put="):
        k = winreg.CreateKey(H, loc + r"\Run")
        winreg.SetValueEx(k, "GitSwitch", 0, winreg.REG_SZ, what[4:])
        print(json.dumps(out)); sys.exit()
    if what == "approved":
        try:
            k = winreg.OpenKey(H, loc + r"\StartupApproved\Run")
            print(json.dumps({"first": winreg.QueryValueEx(k, "GitSwitch")[0][0]}))
        except OSError:
            print(json.dumps({"first": None}))
        sys.exit()
    try:
        k = winreg.OpenKey(H, loc + r"\Run")
        value, kind = winreg.QueryValueEx(k, "GitSwitch")
        out["stamp"] = winreg.QueryInfoKey(k)[2]
    except OSError:
        print(json.dumps(out)); sys.exit()
    out["exists"] = True
    out["raw"] = value
    if kind != winreg.REG_SZ or not value.startswith('"'):
        out["why"] = "not a quoted command"
    else:
        prog, rest = value[1:].split('"', 1)
        out["ok"] = same(prog) and rest.split() == ["--startup"]
        out["why"] = "" if out["ok"] else f"names {prog!r} with {rest!r}"
elif plat == "macos":
    import plistlib
    path = os.path.join(loc, "com.gitswitch.app.startup.plist")
    if what.startswith("put="):
        os.makedirs(loc, exist_ok=True)
        with open(path, "wb") as f:
            plistlib.dump({"Label": "com.gitswitch.app.startup", "ProgramArguments": [what[4:], "--startup"], "RunAtLoad": True}, f)
        print(json.dumps(out)); sys.exit()
    if not os.path.exists(path):
        print(json.dumps(out)); sys.exit()
    out["exists"] = True
    out["stamp"] = os.stat(path).st_mtime_ns
    with open(path, "rb") as f:
        d = plistlib.load(f)
    args = d.get("ProgramArguments", [])
    checks = {
        "label": d.get("Label") == "com.gitswitch.app.startup",
        "program": bool(args) and same(args[0]),
        "flag": args[1:] == ["--startup"],
        "at load": d.get("RunAtLoad") is True,
        "aqua": d.get("LimitLoadToSessionType") == "Aqua",
    }
    out["ok"] = all(checks.values())
    out["why"] = ", ".join(k for k, good in checks.items() if not good)
else:
    path = os.path.join(loc, "gitswitch.desktop")
    if what.startswith("put="):
        os.makedirs(loc, exist_ok=True)
        with open(path, "w") as f:
            f.write(f"[Desktop Entry]\nType=Application\nName=GitSwitch\nExec={what[4:]} --startup\n")
        print(json.dumps(out)); sys.exit()
    if not os.path.exists(path):
        print(json.dumps(out)); sys.exit()
    out["exists"] = True
    out["stamp"] = os.stat(path).st_mtime_ns
    keys = {}
    for line in open(path):
        if "=" in line and not line.startswith("["):
            k, val = line.rstrip("\n").split("=", 1)
            keys[k] = val
    # The file's string escapes first (\\ is \), then the Exec quoting.
    exec_args = shlex.split(keys.get("Exec", "").replace("\\\\", "\\"))
    checks = {
        "type": keys.get("Type") == "Application",
        "program": bool(exec_args) and same(exec_args[0]),
        "flag": exec_args[1:] == ["--startup"],
        "tryexec": same(keys.get("TryExec", "").replace("\\\\", "\\")),
        "enabled": keys.get("X-GNOME-Autostart-enabled") == "true",
    }
    out["ok"] = all(checks.values())
    out["why"] = ", ".join(k for k, good in checks.items() if not good)
print(json.dumps(out))
EOF
}
field() { "$PY" -c "import sys,json; print(json.load(sys.stdin).get('$1'))"; }

entry clean >/dev/null

section "Nothing chosen: nothing starts"
OUT=$(snap)
ok "the tab says it is off" "$(printf '%s' "$OUT" | v "(v['startup']['on'], v['startup']['active'], v['startup']['error'], v['settings']['start_at_login'])")" "(False, False, None, False)"
ok "and there is no entry" "$(entry | field exists)" "False"

section "Asked for with nothing shown: refused by the settings themselves"
OUT=$(save '{"tray":false,"bar":false,"start_at_login":true}')
ok "the setting comes back off (it would start unseen)" "$(printf '%s' "$OUT" | v "v['start_at_login']")" "False"
ok "no entry is made" "$(entry | field exists)" "False"

section "Ticked, with the speed shown: the entry this system keeps"
OUT=$(save '{"tray":true,"start_at_login":true}')
ok "the setting is on" "$(printf '%s' "$OUT" | v "(v['tray'], v['start_at_login'])")" "(True, True)"
E=$(entry)
ok "the entry starts this program with --startup" "$(printf '%s' "$E" | field ok) $(printf '%s' "$E" | field why)" "True "
ok "the tab says so" "$(snap | v "(v['startup']['on'], v['startup']['active'], v['startup']['blocked'], v['startup']['error'])")" "(True, True, None, None)"
case "$PLAT" in
  macos)
    ok "  plutil finds the LaunchAgent sound" "$(plutil -lint "$LOC/com.gitswitch.app.startup.plist" 2>&1)" "OK" ;;
  linux)
    if command -v desktop-file-validate >/dev/null 2>&1; then
      ok "  desktop-file-validate finds the entry sound" "$(desktop-file-validate "$LOC/gitswitch.desktop" 2>&1; echo "exit $?")" "exit 0"
    else
      echo "  (desktop-file-validate is not installed: the entry's format is checked above only)"
    fi ;;
esac
STAMP=$(entry | field stamp)
sleep 1
save '{"tray":true,"start_at_login":true}' >/dev/null
ok "ticked again: the entry is not written again" "$(entry | field stamp)" "$STAMP"

section "The program moved: the entry is corrected"
if [ "$PLAT" = windows ]; then OLD='"C:\Old Place\gitswitch.exe" --startup'; else OLD="/old/place/gitswitch"; fi
entry "put=$OLD" >/dev/null
ok "an entry naming another program is not this one's" "$(entry | field ok)" "False"
ok "  the tab shows it as not set up" "$(snap | v "(v['startup']['on'], v['startup']['active'])")" "(True, False)"
save '{"tray":true,"start_at_login":true}' >/dev/null
ok "saving the setting puts it right" "$(entry | field ok)" "True"

if [ "$PLAT" = windows ]; then
  section "Windows' Startup apps turned it off: said, and left alone"
  entry "approve=03" >/dev/null
  OUT=$(snap)
  ok "the tab says Startup apps has it off" "$(printf '%s' "$OUT" | v "v['startup']['blocked']")" "Startup apps has GitSwitch turned off"
  save '{"tray":true,"start_at_login":true}' >/dev/null
  ok "saving again does not switch it back on" "$(entry approved | field first)" "3"
  ok "  and the entry itself stays" "$(entry | field ok)" "True"
  entry "approve=02" >/dev/null
  ok "switched on there again: nothing to say" "$(snap | v "v['startup']['blocked']")" "None"
fi

section "What a start at sign-in does with the window"
ok "started at sign-in with the speed shown: no window" "$(pinvoke netspeed_start_hidden '{"argv":["gitswitch","--startup"]}' | v "(v['at_login'], v['hidden'])")" "(True, True)"
ok "started by hand: the window" "$(pinvoke netspeed_start_hidden '{"argv":["gitswitch"]}' | v "(v['at_login'], v['hidden'])")" "(False, False)"

section "Uninstalling knows about it"
ok "the uninstall plan lists it" "$(pinvoke uninstall_plan '{}' | v "v['startup']")" "True"

section "The speed no longer shown: the entry goes with it"
if [ "$PLAT" != windows ]; then
  save '{"tray":false,"bar":true,"start_at_login":true}' >/dev/null
  ok "the floating bar alone still counts as shown: it stays" "$(entry | field ok)" "True"
fi
OUT=$(save '{"tray":false,"bar":false,"start_at_login":true}')
ok "the setting turns off with the display" "$(printf '%s' "$OUT" | v "v['start_at_login']")" "False"
ok "the entry is gone" "$(entry | field exists)" "False"
ok "the tab says it is off" "$(snap | v "(v['startup']['on'], v['startup']['active'])")" "(False, False)"
ok "a sign-in start with nothing to show opens the window" "$(pinvoke netspeed_start_hidden '{"argv":["gitswitch","--startup"]}' | v "v['hidden']")" "False"
ok "the uninstall plan has nothing to take back" "$(pinvoke uninstall_plan '{}' | v "v['startup']")" "False"

section "A test build never reaches the real entry"
OUT=$(env -u GITSWITCH_AUTOSTART_KEY -u GITSWITCH_AUTOSTART_DIR bash -c 'source "$1"; PROBE_OP=invoke PROBE_CMD=netspeed_set_settings PROBE_JSON="{\"settings\":{\"tray\":true,\"start_at_login\":true}}" "$2" probe --ignored --nocapture 2>/dev/null >/dev/null; PROBE_OP=invoke PROBE_CMD=netspeed_snapshot PROBE_JSON="{}" "$2" probe --ignored --nocapture 2>/dev/null | sed -n "s/^PROBE_OUT //p"' _ "$VERIFY_DIR/lib.sh" "$BIN")
ok "without the sandbox's place it writes nowhere and says so" "$(printf '%s' "$OUT" | v "(v['startup']['active'], 'could not be found' in (v['startup']['error'] or ''))")" "(False, True)"
save '{"tray":false,"start_at_login":false}' >/dev/null

if [ "${NETSPEED_LIVE:-}" = 1 ]; then
  section "The speed shown is the real one (a 50 MB download)"
  SECS="${NETSPEED_LIVE_SECONDS:-30}"; BYTES=50000000
  W="$SB/watch.json"
  ( PROBE_OP=invoke PROBE_CMD=netspeed_watch PROBE_JSON="{\"seconds\":$SECS}" "$BIN" probe --ignored --nocapture 2>/dev/null | sed -n 's/^PROBE_OUT //p' > "$W" ) &
  WATCH=$!
  sleep 2
  CURL=$(curl -s -o /dev/null -w '%{http_code} %{size_download} %{time_total} %{speed_download}' "https://speed.cloudflare.com/__down?bytes=$BYTES")
  wait "$WATCH"
  read -r ID NAME <<<"$("$PY" -c 'import sys,json; a=json.load(open(sys.argv[1]))["value"].get("adapter") or {}; print(a.get("id","-"), a.get("name","-"))' "$W")"
  case "$PLAT" in
    windows) SYSRX=$(powershell -NoProfile -Command "(Get-NetAdapterStatistics -Name '$NAME').ReceivedBytes" | tr -d '\r') ;;
    macos) SYSRX=$(netstat -ibn | awk -v n="$ID" '$1==n && $3 ~ /Link/ {print $7; exit}') ;;
    *) SYSRX=$(cat "/sys/class/net/$ID/statistics/rx_bytes" 2>/dev/null) ;;
  esac
  REPORT=$("$PY" - "$W" "$CURL" "${SYSRX:-0}" "$SECS" <<'EOF'
import sys, json
d = json.load(open(sys.argv[1]))["value"]
code, size, took, speed = sys.argv[2].split()
size, took, speed, sysrx, secs = int(size), float(took), float(speed), int(sys.argv[3] or 0), float(sys.argv[4])
busy = [s["down"] for s in d["samples"] if s["down"] > 0.25 * speed]
avg = sum(busy) / len(busy) if busy else 0.0
moved = (d.get("moved") or [0, 0])[0]
end = (d.get("counters_at_end") or [0, 0])[0]
# A monitor shows what crosses the adapter, as Task Manager does: curl's
# bytes, plus TLS and TCP, plus whatever a lossy network sent twice (Docker
# Desktop's network resends a fifth on a busy host). So the speed to expect is
# curl's, scaled by the adapter's bytes over curl's.
wire = moved / size if size else 1.0
expect = speed * wire
print(f"download: HTTP {code}, {size} bytes in {took:.2f}s = {speed/1e6:.2f} MB/s; the adapter carried {moved} bytes ({wire:.3f}x), so {expect/1e6:.2f} MB/s across it; GitSwitch showed {avg/1e6:.2f} MB/s over {len(busy)} s; system counter - GitSwitch's last = {sysrx - end}")
print("download", code == "200" and size > 0)
print("speed", bool(busy) and abs(avg - expect) <= 0.15 * expect)
# All of curl's bytes fell in the watch only when the download fit inside it.
print("bytes", (took + 4 > secs) or (moved >= 0.99 * size))
print("counter", sysrx == 0 or abs(sysrx - end) <= max(0.02 * max(moved, 1), 2_000_000))
EOF
)
  echo "  $(printf '%s\n' "$REPORT" | head -1)"
  ok "the download went through" "$(printf '%s\n' "$REPORT" | grep '^download ')" "download True"
  ok "the speed GitSwitch shows is the rate the download crossed the adapter (within 15%)" "$(printf '%s\n' "$REPORT" | grep '^speed ')" "speed True"
  ok "GitSwitch counted at least every byte curl received" "$(printf '%s\n' "$REPORT" | grep '^bytes ')" "bytes True"
  ok "its counter is the system's own count for that adapter" "$(printf '%s\n' "$REPORT" | grep '^counter ')" "counter True"
else
  echo; echo "  (NETSPEED_LIVE=1 also measures the speed during a real 50 MB download)"
fi

entry clean >/dev/null
verify_result
