#!/bin/bash
# Fixture for the live UI ↔ backend harness (live.mjs). Builds the throwaway
# sandbox the built frontend is driven against, then prints one line
# `LIVE_ENV <json>` naming the probe binary, the sandbox and its repositories.
#
# Everything comes from lib.sh's `verify_init`: HOME is redirected to the
# sandbox BEFORE any gitconfig is written, GIT_CONFIG_NOSYSTEM is set, and the
# test binary is located the same way the shell suites locate it. No real
# repository is ever touched; the "remotes" are bare repos inside the sandbox.
set -u
source "$(dirname "$0")/../lib.sh"
verify_init live "Live Tester" "live@example.test"
SB="$(realdir "$SB")"
W="$SB/work"

need() { "$@" || { echo "live-fixture: failed: $*" >&2; exit 1; }; }

# --- the submodule's own history --------------------------------------------
need git init -q --bare "$SB/remote.git"
need git init -q --bare "$SB/sub.git"
need git clone -q "$SB/sub.git" "$SB/seed/sub" 2>/dev/null
printf 'sub v1\n' > "$SB/seed/sub/file.txt"
need git -C "$SB/seed/sub" add -A
need git -C "$SB/seed/sub" commit -qm "sub initial"
need git -C "$SB/seed/sub" push -q origin main 2>/dev/null

# --- work: a few pushed commits, an ignored .env, a mapped submodule ----------
need git clone -q "$SB/remote.git" "$W" 2>/dev/null
printf 'line1\n' > "$W/a.txt"; printf 'shared\n' > "$W/c.txt"
need git -C "$W" add -A; need git -C "$W" commit -qm "first"
printf 'b1\n' > "$W/b.txt"
need git -C "$W" add -A; need git -C "$W" commit -qm "second"
printf '.env\n' > "$W/.gitignore"; printf 'SECRET=1\n' > "$W/.env"
need git -C "$W" add -A; need git -C "$W" commit -qm "third"
need git -C "$W" submodule add -q "$SB/sub.git" vendor/sub 2>/dev/null
need git -C "$W" add -A; need git -C "$W" commit -qm "add submodule"
need git -C "$W" push -q -u origin main 2>/dev/null

# --- another clone of the same remote: the "other developer" ----------------
# Scenarios have it push diverging commits when they need incoming work.
need git clone -q "$SB/remote.git" "$SB/other" 2>/dev/null

# --- a dirty file inside the submodule: what the first scenario stages ------
printf 'edit\n' >> "$W/vendor/sub/file.txt"

printf 'LIVE_ENV %s\n' "$("$PY" -c 'import json,sys; print(json.dumps(dict(bin=sys.argv[1], sb=sys.argv[2], home=sys.argv[3], work=sys.argv[4], other=sys.argv[5], sub=sys.argv[6])))' \
  "$BIN" "$SB" "$HOME" "$W" "$SB/other" "$W/vendor/sub")"
