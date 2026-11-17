#!/bin/bash
# Ports suite: every listening port, who holds it, where it runs from, and
# stopping it — against real sockets, real processes and the real drawer
# shell. Nothing here needs root; what needs an administrator is proven to
# be refused, with its reason.
set -u
source "$(dirname "$0")/lib.sh"
verify_init ports "Ports Tester" "ports@example.test"

IS_WIN=0; command -v cygpath >/dev/null 2>&1 && IS_WIN=1
SB="$(realdir "$SB")"

# A repository in a profile folder: what a listener is attributed to.
mkdir -p "$SB/proj"
"$PY" - "$SB/profile.json" "$SB/proj" <<'JSONPY'
import json, sys
json.dump({"name": "Ports", "git_name": "Ports Tester", "git_email": "ports@example.test", "directories": [sys.argv[2]]}, open(sys.argv[1], "w"))
JSONPY
probe profile_apply "$SB" "$SB/profile.json" >/dev/null
git init -q "$SB/proj/work"; ( cd "$SB/proj/work" && git commit -q --allow-empty -m init )

# A server with a child process: stopping the server must take the child
# with it (the whole process group), and the port must be freed.
cat > "$SB/server.py" <<'PYSRV'
import http.server, socketserver, subprocess, sys, os, signal
if len(sys.argv) > 1 and sys.argv[1] == "ignore":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(3600)"])
socketserver.TCPServer.allow_reuse_address = True
srv = socketserver.TCPServer(("127.0.0.1", 0), http.server.SimpleHTTPRequestHandler)
print(srv.server_address[1], os.getpid(), child.pid, flush=True)
srv.serve_forever()
PYSRV

# Started through the drawer's real shell, so it carries the terminal's
# environment and is a job of its own (a process group the shell leads).
start_server() { # <args> -> PORT PID CHILD
  : > "$SB/server.out"
  if [ "$IS_WIN" = 1 ]; then
    # cmd.exe cannot run an MSYS path (/c/…: "Invalid switch"), and a server
    # attached to the drawer's console ends with it when the probe closes the
    # shell. pythonw has no console, so it outlives the shell as nohup does.
    local pyw; pyw="$("$PY" -c 'import os,sys;print(os.path.join(os.path.dirname(sys.executable),"pythonw.exe"))')"
    probe term_run "$SB/proj/work" "start \"\" /B \"$pyw\" $SB/server.py $1 > $SB/server.out" >/dev/null 2>&1
  else
    # Input from /dev/null: GNU nohup on a terminal's input says "nohup:
    # ignoring input" first, into the very file the port is read from.
    probe term_run "$SB/proj/work" "nohup $PY $SB/server.py $1 < /dev/null > $SB/server.out 2>&1 & disown" >/dev/null 2>&1
  fi
  for i in $(seq 1 40); do [ -s "$SB/server.out" ] && break; sleep 0.25; done
  cat "$SB/server.out"
}
# Git Bash's kill knows MSYS pids only; a process cmd.exe started has none.
alive() { if [ "$IS_WIN" = 1 ]; then tasklist //FI "PID eq $1" //FO CSV //NH 2>/dev/null | grep -q "\"$1\""; else kill -0 "$1" 2>/dev/null; fi; }

section "A listener is named with its process, its folder and its repository"
read -r PORT PID CHILD < <(start_server "")
# Not "running"/"not running": the second contains the first.
ok "(a server started from the drawer's shell)" "$([ -n "${PID:-}" ] && alive "$PID" && echo started || echo "NOT STARTED: $(cat "$SB/server.out")")" "started"
L=$(pinvoke ports_list '{}')
ROW=$(printf '%s' "$L" | "$PY" -c 'import sys,json;rows=json.load(sys.stdin)["value"]["rows"];r=[x for x in rows if x["pid"]==int(sys.argv[1])];print(json.dumps(r[0]) if r else "{}")' "$PID")
ok "the server is listed on its port" "$(printf '%s' "$ROW" | jqf port):$(printf '%s' "$ROW" | jqf proto)" "$PORT:tcp"
if [ "$IS_WIN" = 0 ]; then
  ok "  with the folder it runs from" "$(printf '%s' "$ROW" | jqf cwd)" "$SB/proj/work"
  ok "  attributed to the repository and its profile" "$(printf '%s' "$ROW" | jqf clone.name):$(printf '%s' "$ROW" | jqf clone.profile_name)" "work:Ports"
  ok "  marked as started from a GitSwitch terminal" "$(printf '%s' "$ROW" | jqf ours)" "True"
else
  echo "  SKIPPED: a process's folder and environment are not read on Windows (ports::gather says so), so its folder, repository and 'ours' cannot be known there"
fi
ok "  on localhost, and stoppable" "$(printf '%s' "$ROW" | jqf scope):$(printf '%s' "$ROW" | jqf killable)" "loopback:True"
NAME=$(printf '%s' "$ROW" | jqf name)

section "What may not be stopped is refused, with the reason"
ok "pid 1 is a system process" "$(pinvoke ports_kill '{"pid":1,"force":false,"name":"launchd"}' | jqf error)" "a system process (pid 1)"
if [ "$IS_WIN" = 0 ]; then
  ROOTPID=$(ps -axo pid=,uid=,comm= | awk '$2==0 && $1>=100 {print $1; exit}')
  ROOTNAME=$(ps -o comm= -p "$ROOTPID" | xargs basename)
  # bash 3.2 brace-expands inline JSON inside $( ); the string is assigned first.
  J="{\"pid\":$ROOTPID,\"force\":false,\"name\":\"$ROOTNAME\"}"
  ok "a root-owned process needs an administrator" "$(pinvoke ports_kill "$J" | jqf error)" "needs an administrator"
fi
J="{\"pid\":$PID,\"force\":false,\"name\":\"not-this-program\"}"
ok "a pid whose process is now something else is refused — the list is stale" "$(pinvoke ports_kill "$J" | jqf error)" "the list is stale"
ok "GitSwitch never stops itself" "$(probe ports_kill_self "$SB/proj/work")" "GitSwitch itself"
ok "  nor the drawer's own shell" "$(probe ports_kill_shell "$SB/proj/work")" "terminal drawer's own shell"

section "Stop ends the server and its child; a port is freed"
OUT=$(pinvoke ports_kill "{\"pid\":$PID,\"force\":false,\"name\":\"$NAME\"}")
if [ "$IS_WIN" = 0 ]; then
  ok "Stop ends it" "$(printf '%s' "$OUT" | jqf value.ended):$(printf '%s' "$OUT" | jqf value.how)" "True:term"
else
  # Stop is taskkill without /F, which only asks a program's windows to
  # close: a server with none (python, node) can only be ended forcefully,
  # and Stop says so.
  ok "Stop asks politely, and says when that was not enough" "$(printf '%s' "$OUT" | jqf value.ended):$(printf '%s' "$OUT" | jqf value.how)" "False:taskkill"
  ok "  naming Force stop" "$(printf '%s' "$OUT" | jqf value.message)" "Force stop"
  OUT=$(pinvoke ports_kill "{\"pid\":$PID,\"force\":true,\"name\":\"$NAME\"}")
  ok "Force stop ends it" "$(printf '%s' "$OUT" | jqf value.ended):$(printf '%s' "$OUT" | jqf value.how)" "True:taskkill-force"
fi
sleep 1
ok "  the server is gone" "$(alive "$PID" && echo alive || echo gone)" "gone"
if [ "$IS_WIN" = 0 ]; then
  ok "  and its child died with it — the process group was signalled" "$(alive "$CHILD" && echo alive || echo gone):$(printf '%s' "$OUT" | jqf value.group)" "gone:True"
  alive "$CHILD" && kill "$CHILD" 2>/dev/null
else
  ok "  and its child died with it — taskkill /T ends the tree" "$(alive "$CHILD" && echo alive || echo gone)" "gone"
fi
ok "  the port is no longer listed" "$(pinvoke ports_list '{}' | "$PY" -c 'import sys,json;rows=json.load(sys.stdin)["value"]["rows"];print(any(x["port"]==int(sys.argv[1]) for x in rows))' "$PORT")" "False"

if [ "$IS_WIN" = 0 ]; then
section "A process that ignores Stop says so; Force stop ends it"
read -r PORT PID CHILD < <(start_server "ignore")
ok "(a server that ignores SIGTERM)" "$(alive "$PID" && echo running || echo "not running")" "running"
NAME=$(pinvoke ports_list '{}' | "$PY" -c 'import sys,json;rows=json.load(sys.stdin)["value"]["rows"];r=[x for x in rows if x["pid"]==int(sys.argv[1])];print(r[0]["name"] if r else "")' "$PID")
OUT=$(pinvoke ports_kill "{\"pid\":$PID,\"force\":false,\"name\":\"$NAME\"}")
ok "Stop reports that it was ignored, and names Force stop" "$(printf '%s' "$OUT" | jqf value.ended):$(printf '%s' "$OUT" | jqf value.message)" "False:"
ok "  (the message says so)" "$(printf '%s' "$OUT" | jqf value.message)" "Force stop"
OUT=$(pinvoke ports_kill "{\"pid\":$PID,\"force\":true,\"name\":\"$NAME\"}")
ok "Force stop ends it" "$(printf '%s' "$OUT" | jqf value.ended):$(printf '%s' "$OUT" | jqf value.how)" "True:kill"
sleep 1
ok "  the server is gone" "$(alive "$PID" && echo alive || echo gone)" "gone"
alive "$CHILD" && kill "$CHILD" 2>/dev/null
fi

section "A port published by Docker is a container, stopped through Docker"
DOCKER_UP=0
if command -v docker >/dev/null 2>&1; then
  ( docker ps -q >/dev/null 2>&1 ) & DP=$!
  for i in 1 2 3 4 5 6; do kill -0 "$DP" 2>/dev/null || break; sleep 0.5; done
  if kill -0 "$DP" 2>/dev/null; then kill "$DP" 2>/dev/null; else wait "$DP" 2>/dev/null && DOCKER_UP=1; fi
fi
IMAGE="${GITSWITCH_VERIFY_DOCKER_IMAGE:-python:3-alpine}"
if [ "$DOCKER_UP" = 1 ] && docker image inspect "$IMAGE" >/dev/null 2>&1; then
  CID=$(docker run -d --rm -p 127.0.0.1:0:8000 -l com.docker.compose.project=portsverify -l "com.docker.compose.project.working_dir=$SB/proj/work" -l com.docker.compose.service=web "$IMAGE" python -m http.server 8000)
  sleep 2
  HPORT=$(docker port "$CID" 8000 | head -1 | awk -F: '{print $NF}')
  ROW=$(pinvoke ports_list '{}' | "$PY" -c 'import sys,json;rows=json.load(sys.stdin)["value"]["rows"];r=[x for x in rows if x["port"]==int(sys.argv[1]) and x["container"]];print(json.dumps(r[0]) if r else "{}")' "$HPORT")
  ok "a container's published port names the container, its project and the repository" "$(printf '%s' "$ROW" | jqf container.project):$(printf '%s' "$ROW" | jqf clone.name)" "portsverify:work"
  ok "  and is never offered as a process to stop" "$(printf '%s' "$ROW" | jqf killable):$(printf '%s' "$ROW" | jqf why_not)" "False:Published by Docker"
  OUT=$(pinvoke ports_container_stop "{\"id\":\"$CID\",\"project\":null}")
  ok "Stop container stops it" "$(printf '%s' "$OUT" | jqf value.message)" "Stopped"
  sleep 1
  ok "  and Docker no longer lists it" "$(docker ps -q --no-trunc | grep -c "$CID" || true)" "0"
  ok "an id that is not an id is refused before anything runs" "$(pinvoke ports_container_stop '{"id":"f50b929f94ba; rm -rf /","project":null}' | jqf error)" "is not a container id"
  ok "  and so is a project name that is not one" "$(pinvoke ports_container_stop '{"id":"f50b929f94ba","project":"Argos; true"}' | jqf error)" "is not a Compose project name"
else
  echo "  SKIPPED: Docker is not running or the image $IMAGE is not present — the container checks need it"
fi

verify_result
