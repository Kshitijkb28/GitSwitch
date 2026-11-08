#!/bin/bash
# Workspaces: two checkouts of one project told apart — each with an id in its
# own .git/config, a port offset, and (for every copy but the first) a Compose
# project name of its own — exported into the drawer's real shell, so what is
# started there inherits them. Against real clones of a real remote, through
# the same backend the app calls.
set -u
source "$(dirname "$0")/lib.sh"
verify_init workspace "Workspace Tester" "ws@example.test"

IS_WIN=0; command -v cygpath >/dev/null 2>&1 && IS_WIN=1
SB="$(realdir "$SB")"
P="$SB/proj"

# A profile folder: where the copies live, and what they are attributed to.
mkdir -p "$P"
# A folder that is no repository. The sandbox itself may sit inside one (a
# checkout of GitSwitch, when GITSWITCH_VERIFY_TMP points into it), and git
# would then find that repository above it — so this one lives in the system's
# temp folder, which is inside none. In its native form: on Windows mktemp
# answers /tmp/… (or /c/…), which the backend cannot open.
PLAIN="$(realdir "$(mktemp -d "${TMPDIR:-/tmp}/gitswitch-plain.XXXXXX")")"
trap 'rm -rf "$PLAIN"' EXIT
"$PY" - "$SB/profile.json" "$P" <<'JSONPY'
import json, sys
json.dump({"name": "Workspaces", "git_name": "Workspace Tester", "git_email": "ws@example.test", "directories": [sys.argv[2]]}, open(sys.argv[1], "w"))
JSONPY
probe profile_apply "$SB" "$SB/profile.json" >/dev/null

# One project with a commit, and a second, unrelated remote.
git init -q --bare "$SB/remote.git"
git init -q --bare "$SB/other.git"
git clone -q "$SB/remote.git" "$SB/seed" 2>/dev/null
( cd "$SB/seed" && echo hello > README.md && git add -A && git commit -qm first && git push -q origin main 2>/dev/null )
rm -rf "$SB/seed"
git clone -q "$SB/other.git" "$SB/proj/other" 2>/dev/null
( cd "$SB/proj/other" && echo other > README.md && git add -A && git commit -qm first && git push -q origin main 2>/dev/null )

# The two copies: zeta first, alpha a second later — the reflog's seconds are
# what decides which copy is the older one, not the folder names' order.
git clone -q "$SB/remote.git" "$P/zeta" 2>/dev/null
sleep 1.2
git clone -q "$SB/remote.git" "$P/alpha" 2>/dev/null

winfo() { pinvoke workspace_info "{\"path\":\"$1\"}"; }
# copies <json> -> one line per copy, by offset: label:offset:is_this:kind
copies() { printf '%s' "$1" | "$PY" -c '
import sys,json
d=json.load(sys.stdin).get("value") or {}
for c in d.get("copies", []): print("%s:%s:%s:%s" % (c["label"], c["offset"], c["is_this"], c["kind"]))'; }
ncopies() { copies "$1" | grep -c . || true; }
# envvar <json from workspace_env> <NAME> -> the value, or <unset>
envvar() { printf '%s' "$1" | "$PY" -c 'import sys,json;d=json.load(sys.stdin);print(dict(d["env"]).get(sys.argv[1], "<unset>"))' "$2"; }
# What a person reads: ConPTY often ends a line by moving the cursor to the
# next row's first column instead of printing CR LF, and sets the window title
# (OSC) as commands run; neither is text on the screen.
strip() { "$PY" -c "
import sys,json,re
d=json.load(sys.stdin)
o=re.sub(r'\x1b\[[0-9]+;1H', '\n', d.get(sys.argv[1], ''))
print(re.sub(r'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\\\)?|\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][B0]|\r', '', o))" "$1"; }
# term <repo> <line> -> what a real shell opened there printed
term() { probe term_run "$1" "$2" | strip output; }
prelude() { probe term_run_prelude "$1" "true" | strip prelude; }
# say <sh form> <cmd form> -> the line for the shell under test
say() { if [ "$IS_WIN" = 1 ]; then printf '%s' "$2"; else printf '%s' "$1"; fi; }
cfg() { git -C "$1" config --local "$2" 2>/dev/null || echo "<unset>"; }

section "Two clones of one remote are two copies, each told apart"
I=$(winfo "$P/zeta")
ok "two copies of the remote are found among the profile's folders" "$(ncopies "$I")" "2"
ok "zeta, the older clone, keeps offset 0 — and is this one" "$(copies "$I")" "zeta:0:True:clone"
ok "  alpha, the newer clone, takes offset 100" "$(copies "$I")" "alpha:100:False:clone"
ZID=$(printf '%s' "$I" | jqf value.id)
ok "the id is eight hex characters" "$(printf '%s' "$ZID" | grep -cE '^[0-9a-f]{8}$')" "1"
ok "  written into zeta's own .git/config, never pushed" "$(cfg "$P/zeta" gitswitch.workspace)" "$ZID"
ok "  with its offset beside it" "$(cfg "$P/zeta" gitswitch.portOffset)" "0"
ok "the first copy keeps Compose's own project name" "$(printf '%s' "$I" | jqf value.compose_project)" "None"
ok "  and stores none" "$(cfg "$P/zeta" gitswitch.composeProject)" "<unset>"
A=$(winfo "$P/alpha")
AID=$(printf '%s' "$A" | jqf value.id)
ok "alpha has an id of its own" "$([ "$AID" != "$ZID" ] && [ -n "$AID" ] && echo different || echo same)" "different"
ok "  and a Compose project of its own, named after the folder and the id" "$(printf '%s' "$A" | jqf value.compose_project)" "alpha-$AID"
ok "  stored in its .git/config" "$(cfg "$P/alpha" gitswitch.composeProject)" "alpha-$AID"
ok "the remote both are copies of is named" "$(printf '%s' "$I" | jqf value.remote)" "$SB/remote.git"
ok "a read with nothing to assign writes nothing new" "$(winfo "$P/zeta" | jqf value.warnings)" "[]"
ok "the page is told what is not automated" "$(printf '%s' "$I" | jqf value.not_automated)" "never switches a branch, edits a port in your project, or renames containers"

section "The id and the offset follow a renamed folder"
mv "$P/alpha" "$P/alpha-moved"
M=$(winfo "$P/alpha-moved")
ok "the same id after the move" "$(printf '%s' "$M" | jqf value.id)" "$AID"
ok "  the same offset" "$(printf '%s' "$M" | jqf value.offset)" "100"
ok "  the same Compose project — its containers and volumes stay reachable" "$(printf '%s' "$M" | jqf value.compose_project)" "alpha-$AID"
ok "  and the copies list uses the new name" "$(copies "$M")" "alpha-moved:100:True:clone"

section "A third clone takes the next offset; a shell opened there inherits the workspace"
git clone -q "$SB/remote.git" "$P/gamma" 2>/dev/null
G=$(winfo "$P/gamma")
GID=$(printf '%s' "$G" | jqf value.id)
ok "the third copy gets offset 200" "$(printf '%s' "$G" | jqf value.offset)" "200"
ok "  three copies now" "$(ncopies "$G")" "3"
E=$(probe workspace_env "$P/gamma")
ok "the drawer exports the id" "$(envvar "$E" GITSWITCH_WORKSPACE)" "$GID"
ok "  the offset, under GitSwitch's own name (a project's PORT_OFFSET is never overridden)" "$(envvar "$E" GITSWITCH_PORT_OFFSET)" "200"
ok "  the Compose project" "$(envvar "$E" COMPOSE_PROJECT_NAME)" "gamma-$GID"
ok "  the workspace's folder and name" "$(envvar "$E" GITSWITCH_WORKSPACE_PATH):$(envvar "$E" GITSWITCH_WORKSPACE_NAME)" "$P/gamma:gamma"
OUT=$(term "$P/gamma" "$(say 'echo "[$GITSWITCH_WORKSPACE;$GITSWITCH_PORT_OFFSET;$COMPOSE_PROJECT_NAME;$GITSWITCH_WORKSPACE_NAME]"' 'echo [%GITSWITCH_WORKSPACE%;%GITSWITCH_PORT_OFFSET%;%COMPOSE_PROJECT_NAME%;%GITSWITCH_WORKSPACE_NAME%]')")
ok "a real shell opened in the copy sees all of it" "$OUT" "[$GID;200;gamma-$GID;gamma]"
# cmd.exe expands %VAR% in the typed line before cmd /c runs; %^VAR% reaches
# the child as %VAR%, so it is the child that expands it.
OUT=$(term "$P/gamma" "$(say 'sh -c '"'"'echo child=$GITSWITCH_PORT_OFFSET'"'"'' 'cmd /c echo child=%^GITSWITCH_PORT_OFFSET%')")
ok "  and so does a process it starts — an agent, a dev server" "$OUT" "child=200"

section "The first copy exports no Compose name; the banner says what the shell got"
# cmd.exe prints an undefined %VAR% as typed, never as nothing.
OUT=$(term "$P/zeta" "$(say 'echo "[$COMPOSE_PROJECT_NAME]"' 'if defined COMPOSE_PROJECT_NAME (echo [%COMPOSE_PROJECT_NAME%]) else (echo [])')")
ok "zeta's shell has no COMPOSE_PROJECT_NAME — its containers keep their names" "$(printf '%s' "$OUT" | grep -c '^\[\]$' || true)" "1"
B=$(prelude "$P/zeta")
ok "the first line of zeta's shell is its banner" "$B" "GitSwitch workspace #$ZID · zeta · port offset 0 (\$GITSWITCH_PORT_OFFSET)"
ok "  with no Compose project to speak of" "$(printf '%s' "$B" | grep -c 'Compose project' || true)" "0"
B=$(prelude "$P/gamma")
ok "gamma's banner names its Compose project as this copy's own" "$B" "Compose project gamma-$GID (this copy's own)"

section "The amber note: one branch checked out in two copies, with changes in both"
echo change >> "$P/zeta/README.md"
echo change >> "$P/alpha-moved/README.md"
I=$(winfo "$P/zeta")
ok "both on main with a tracked change each → the note, naming both" "$(printf '%s' "$I" | jqf value.note.text)" "Both copies have main checked out with uncommitted changes (zeta: 1 file, alpha-moved: 1 file). Commits made in both will diverge and the second push will be refused — work on a branch per copy."
ok "  the note is amber" "$(printf '%s' "$I" | jqf value.note.level)" "amber"
git -C "$P/alpha-moved" checkout -q -b feature
ok "alpha-moved on its own branch → no note" "$(winfo "$P/zeta" | jqf value.note)" "None"
git -C "$P/alpha-moved" checkout -q main
git -C "$P/alpha-moved" checkout -q -- README.md
echo new > "$P/alpha-moved/scratch.txt"
ok "an untracked file alone is not a change that diverges → no note" "$(winfo "$P/zeta" | jqf value.note)" "None"
git -C "$P/zeta" checkout -q -- README.md
rm -f "$P/alpha-moved/scratch.txt"

section "A submodule checkout of the same remote is its superproject's workspace, not another copy"
git -C "$P/zeta" -c protocol.file.allow=always submodule add -q "$SB/remote.git" vendor/lib 2>/dev/null
git -C "$P/zeta" commit -qm "add submodule"
S=$(winfo "$P/zeta/vendor/lib")
ok "asked about the submodule, the answer is the superproject" "$(printf '%s' "$S" | jqf value.path)" "$P/zeta"
ok "  with the superproject's id" "$(printf '%s' "$S" | jqf value.id)" "$ZID"
ok "  and the submodule is not counted as a copy" "$(ncopies "$S")" "3"
E=$(probe workspace_env "$P/zeta/vendor/lib")
ok "a shell opened inside the submodule exports the superproject's folder" "$(envvar "$E" GITSWITCH_WORKSPACE_PATH)" "$P/zeta"

section "A linked worktree is a workspace of its own, its keys in the common config"
git -C "$P/zeta" worktree add -q "$P/zeta-wt" -b wt-branch 2>/dev/null
W=$(winfo "$P/zeta-wt")
WID=$(printf '%s' "$W" | jqf value.id)
ok "the worktree is a workspace of kind worktree" "$(printf '%s' "$W" | jqf value.kind)" "worktree"
ok "  with an id of its own" "$([ "$WID" != "$ZID" ] && [ -n "$WID" ] && echo different || echo same)" "different"
ok "  and the next free offset" "$(printf '%s' "$W" | jqf value.offset)" "300"
ok "  its offset is stored under its name in the main checkout's config" "$(cfg "$P/zeta" gitswitch.worktree.zeta-wt.portOffset)" "300"
ok "  the main checkout's own offset is untouched" "$(cfg "$P/zeta" gitswitch.portOffset)" "0"
ok "  and no second id was written into the shared config" "$(cfg "$P/zeta-wt" gitswitch.workspace)" "$ZID"
ok "  four copies now" "$(ncopies "$W")" "4"
E=$(probe workspace_env "$P/zeta-wt")
ok "a shell in the worktree gets the worktree's id, folder and offset" "$(envvar "$E" GITSWITCH_WORKSPACE):$(envvar "$E" GITSWITCH_WORKSPACE_PATH):$(envvar "$E" GITSWITCH_PORT_OFFSET)" "$WID:$P/zeta-wt:300"

section "A folder copied with its .git is given an identity of its own"
cp -R "$P/gamma" "$P/gamma-dup"
D=$(winfo "$P/gamma-dup")
DID=$(printf '%s' "$D" | jqf value.id)
ok "the copy's id differs from gamma's" "$([ "$DID" != "$GID" ] && [ -n "$DID" ] && echo different || echo same)" "different"
ok "  and it is said why" "$(printf '%s' "$D" | jqf value.warnings)" "gamma-dup carried the same workspace id as gamma (a folder copied with its .git); it was given its own."
ok "  it takes a free offset" "$(printf '%s' "$D" | jqf value.offset)" "400"
ok "  and a Compose project of its own, not gamma's" "$(cfg "$P/gamma-dup" gitswitch.composeProject)" "gamma-dup-$DID"
ok "gamma keeps its id" "$(cfg "$P/gamma" gitswitch.workspace)" "$GID"
ok "  and its offset" "$(cfg "$P/gamma" gitswitch.portOffset)" "200"

section "Two copies with one folder name are told apart by their parents"
mkdir -p "$P/x" "$P/y"
git clone -q "$SB/remote.git" "$P/x/argos" 2>/dev/null
git clone -q "$SB/remote.git" "$P/y/argos" 2>/dev/null
X=$(winfo "$P/x/argos")
ok "the labels grow a parent" "$(copies "$X" | grep -c 'x/argos:\|y/argos:' || true)" "2"
ok "  while the others stay short" "$(copies "$X")" "zeta:0:False:clone"

section "Nothing is written to a repository of another remote"
ok "the clone of the other remote has no gitswitch keys after all of this" "$(git -C "$P/other" config --local --get-regexp '^gitswitch\.' 2>/dev/null || echo none)" "none"
O=$(winfo "$P/other")
ok "  asked about, it is a single copy" "$(ncopies "$O")" "1"
ok "  with no note" "$(printf '%s' "$O" | jqf value.note)" "None"

section "Choosing an offset: a clash is refused, a free one accepted, zero removes the Compose name"
R=$(pinvoke workspace_set_offset "{\"path\":\"$P/alpha-moved\",\"offset\":200}")
ok "an offset another copy uses is refused, naming it" "$(printf '%s' "$R" | jqf error)" "gamma already uses port offset 200. Pick another, or change it there first."
ok "  and nothing changed" "$(cfg "$P/alpha-moved" gitswitch.portOffset)" "100"
R=$(pinvoke workspace_set_offset "{\"path\":\"$P/alpha-moved\",\"offset\":700}")
ok "a free offset is taken" "$(printf '%s' "$R" | jqf value.offset)" "700"
ok "  stored" "$(cfg "$P/alpha-moved" gitswitch.portOffset)" "700"
ok "  the Compose project stays this copy's own" "$(printf '%s' "$R" | jqf value.compose_project)" "alpha-moved-$AID"
R=$(pinvoke workspace_set_offset "{\"path\":\"$P/alpha-moved\",\"offset\":70000}")
ok "an offset beyond the port range is refused" "$(printf '%s' "$R" | jqf error)" "A port offset is a whole number from 0 to 60000"
R=$(pinvoke workspace_set_offset "{\"path\":\"$P/other\",\"offset\":100}")
ok "a single copy given an offset gets a Compose project" "$(printf '%s' "$R" | jqf value.compose_project)" "other-"
R=$(pinvoke workspace_set_offset "{\"path\":\"$P/other\",\"offset\":0}")
ok "  back at 0 it has none" "$(printf '%s' "$R" | jqf value.compose_project)" "None"
ok "  and the key is gone" "$(cfg "$P/other" gitswitch.composeProject)" "<unset>"

section "A folder that is no repository still gets a shell"
E=$(probe workspace_env "$PLAIN")
ok "its id comes from its path" "$(envvar "$E" GITSWITCH_WORKSPACE | grep -cE '^[0-9a-f]{8}$')" "1"
ok "  offset 0, no Compose project" "$(envvar "$E" GITSWITCH_PORT_OFFSET):$(envvar "$E" COMPOSE_PROJECT_NAME)" "0:<unset>"
OUT=$(term "$PLAIN" "$(say 'echo "[$GITSWITCH_PORT_OFFSET]"' 'echo [%GITSWITCH_PORT_OFFSET%]')")
ok "  and the shell opens" "$OUT" "[0]"
ok "the Repository page is told it is no repository" "$(winfo "$PLAIN" | jqf error)" "is not inside a git repository"
ok "  and that a bare repository has no workspace" "$(winfo "$SB/remote.git" | jqf error)" "is a bare repository"

verify_result
