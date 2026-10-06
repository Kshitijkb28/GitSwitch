#!/bin/bash
# Submodule reporting, on the shape that breaks `git submodule status`: a
# superproject with one mapped submodule AND one gitlink that has no
# .gitmodules entry. Throwaway repos, isolated HOME, no network.
set -u
source "$(dirname "$0")/lib.sh"
verify_init subs "Sub Tester" "sub@example.test"

sub() { # json on stdin, <path> <field>
  python3 -c "
import sys,json
d=json.load(sys.stdin)
m=[x for x in d if x['path']=='$1']
print(json.dumps(m[0].get('$2')) if m else 'MISSING')
"; }

# --- three submodule upstreams ----------------------------------------------
for name in mapped orphan moved; do
  git init -q --bare "$SB/$name.git"
  git clone -q "$SB/$name.git" "$SB/${name}_work" 2>/dev/null
  cd "$SB/${name}_work"
  echo "$name v1" > file.txt
  git add -A >/dev/null; git commit -qm "$name initial"; git push -q origin main 2>/dev/null
done

# --- the superproject -------------------------------------------------------
git init -q --bare "$SB/super.git"
git clone -q "$SB/super.git" "$SB/super" 2>/dev/null
cd "$SB/super"
echo root > README.md; git add -A >/dev/null; git commit -qm "root"
git submodule add -q "$SB/mapped.git" mapped 2>/dev/null
git submodule add -q "$SB/orphan.git" orphan 2>/dev/null
git submodule add -q "$SB/moved.git" moved 2>/dev/null
git add -A >/dev/null; git commit -qm "add three submodules"
# Strip 'orphan' from .gitmodules but keep its gitlink — exactly argos's shape.
git config -f .gitmodules --remove-section submodule.orphan
git add .gitmodules >/dev/null; git commit -qm "drop orphan mapping"

section "The shape that breaks git's own tooling"
OUT=$(git submodule status 2>&1); RC=$?
ok "git submodule status really does fatal here" "$OUT" "no submodule mapping found"
ok "  and it aborts rather than listing the rest" "rc=$RC" "rc=128"

section "GitSwitch lists every gitlink anyway"
OUT=$(probe submodules_list "$SB/super")
ok "all three are reported" "$(printf '%s' "$OUT" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)))')" "3"
ok "  the mapped one is named" "$(printf '%s' "$OUT" | sub mapped name)" "mapped"
ok "  the unmapped one is still listed" "$(printf '%s' "$OUT" | sub orphan path)" "orphan"
ok "  and is flagged as having no entry" "$(printf '%s' "$OUT" | sub orphan listed)" "false"
ok "  clean submodules say so" "$(printf '%s' "$OUT" | sub mapped summary)" "up to date with what this repo records"

section "A submodule that moved forward"
cd "$SB/moved_work"
echo "moved v2" >> file.txt; git commit -qam "second"
echo "moved v3" >> file.txt; git commit -qam "third"
git push -q origin main 2>/dev/null
cd "$SB/super/moved"; git fetch -q origin 2>/dev/null; git checkout -q origin/main 2>/dev/null
cd "$SB/super"
OUT=$(probe submodules_list "$SB/super")
ok "the move is measured, not guessed" "$(printf '%s' "$OUT" | sub moved ahead)" "2"
ok "  and described in words" "$(printf '%s' "$OUT" | sub moved summary)" "moved 2 commits ahead"
ok "  with the commit subjects" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin); m=[x for x in d if x['path']=='moved'][0]
print(','.join(c['subject'] for c in m['moved_commits']))")" "third"
ok "  state is 'moved'" "$(printf '%s' "$OUT" | sub moved state)" "moved"
ok "  recorded and checked-out differ" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin); m=[x for x in d if x['path']=='moved'][0]
print('differ' if m['recorded_short'] != m['actual_short'] else 'same')")" "differ"

section "A submodule with its own uncommitted work"
echo "local edit" >> "$SB/super/mapped/file.txt"
printf 'new\n' > "$SB/super/mapped/untracked.txt"
OUT=$(probe submodules_list "$SB/super")
ok "changed files inside are counted" "$(printf '%s' "$OUT" | sub mapped dirty_tracked)" "1"
ok "untracked files inside are counted" "$(printf '%s' "$OUT" | sub mapped dirty_untracked)" "1"
ok "  and stated in the summary" "$(printf '%s' "$OUT" | sub mapped summary)" "1 changed file and 1 untracked file inside"
ok "  state is 'dirty'" "$(printf '%s' "$OUT" | sub mapped state)" "dirty"

section "An uninitialised submodule"
rm -rf "$SB/super/orphan"
mkdir -p "$SB/super/orphan"
OUT=$(probe submodules_list "$SB/super")
ok "an empty gitlink dir is reported as not initialised" "$(printf '%s' "$OUT" | sub orphan state)" "unmapped"
ok "  and says why git can't fetch it" "$(printf '%s' "$OUT" | sub orphan summary)" "no entry in .gitmodules"
ok "  it is not claimed to be initialised" "$(printf '%s' "$OUT" | sub orphan initialised)" "false"

section "The superproject's own file list no longer lies"
OUT=$(probe status "$SB/super")
ok "a submodule row carries no fake line counts" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
subs=[e for e in d['entries'] if e['is_submodule']]
print('none' if all(e['unstaged_added'] is None and e['staged_added'] is None for e in subs) else 'FAKE COUNTS')")" "none"
ok "  and it keeps the recorded commit" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
subs=[e for e in d['entries'] if e['is_submodule']]
print('yes' if subs and all(e['recorded_oid'] for e in subs) else 'no')")" "yes"

section "Clicking a submodule shows the pointer move, not 'nothing changed'"
OUT=$(probe diff "$SB/super" "moved,unstaged")
ok "the diff names both commits" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(' '.join(l['text'] for l in d['lines'] if 'Subproject' in l['text']))")" "Subproject commit"
ok "  and does not claim there are no changes" "$(printf '%s' "$OUT" | python3 -c "
import sys,json;print(json.load(sys.stdin)['empty_reason'])")" "None"

section "Update still works on this repo"
OUT=$(probe submodule "$SB/super")
ok "the unmapped gitlink is reported, not fatal" "$OUT" "up to date"
ok "  and the count of unfetchable ones is honest" "$(printf '%s' "$OUT" | jqf submodules.unlisted)" "1"

section "Initialising one submodule leaves every other one exactly as it was"
# A fresh clone of the superproject: every gitlink is an empty folder, and the
# .gitmodules addresses are the local bare repos above. The Submodules tab's
# "Initialise" buttons call changes_submodule_update with the paths they name.
git clone -q "$SB/super" "$SB/super_fresh" 2>/dev/null
cd "$SB/super_fresh"
ok "the fixture: nothing populated yet" "$(find mapped moved orphan -mindepth 1 | wc -l | tr -d ' ')" "0"
ARGS=$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1],"paths":["moved"]}))' "$SB/super_fresh")
OUT=$(pinvoke changes_submodule_update "$ARGS")
TRACE_INIT=$(probe_trace)      # read right after the call that really populated one
ok "asked for 'moved' only, it reports one of one" "$(printf '%s' "$OUT" | jqf value.headline)" "1 of 1 submodule(s) up to date."
ok "  moved is checked out at the recorded commit" "$(git -C "$SB/super_fresh/moved" rev-parse HEAD)" "$(git rev-parse HEAD:moved)"
ok "  mapped was not touched" "$(find mapped -mindepth 1 | wc -l | tr -d ' ')" "0"
ok "  and is still listed as not initialised" "$(probe submodules_list "$SB/super_fresh" | sub mapped state)" "not-initialised"
ok "  the unmapped gitlink is counted, not fatal" "$(printf '%s' "$OUT" | jqf value.submodules.unlisted)" "1"
# Work inside the populated one, then initialise another: the work must survive.
git -C moved checkout -q -b own-work
echo "own" >> moved/file.txt
git -C moved commit -qam "own work inside moved"
OWN=$(git -C moved rev-parse HEAD)
ARGS=$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1],"paths":["mapped"]}))' "$SB/super_fresh")
OUT=$(pinvoke changes_submodule_update "$ARGS")
ok "asked for 'mapped', it is populated" "$(git -C mapped rev-parse HEAD)" "$(git rev-parse HEAD:mapped)"
ok "  and the commit made inside moved is still checked out" "$(git -C moved rev-parse HEAD)" "$OWN"
git submodule update --quiet -- moved >/dev/null 2>&1
ok "  (git's own update of that path moves it back — which is why the app names paths)" "$(git -C moved rev-parse HEAD)" "$(git rev-parse HEAD:moved)"
ARGS=$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1],"paths":["../escape"]}))' "$SB/super_fresh")
ok "a path outside the repository is refused" "$(pinvoke changes_submodule_update "$ARGS")" "isn't a path inside this repository"
cd "$SB/super"

ok "  and initialising from the Changes page reports no clone progress — no clone is running there" "$TRACE_INIT" "[]"

section "Cloning this superproject names each submodule as it arrives"
mkdir -p "$SB/clones"
OUT=$(probe clone "" "$SB/super,$SB/clones,fresh,submodules")
ok "the clone lists the mapped submodules and fetches them" "$(printf '%s' "$OUT" | jqf submodules.listed):$(printf '%s' "$OUT" | jqf submodules.downloaded)" "2:2"
TRACE=$(probe_trace)
ok "  each was named while it was the one being fetched, in order" "$(printf '%s' "$TRACE" | "$PY" -c 'import sys,json;e=[p for p in json.load(sys.stdin) if p["phase"]=="Submodules" and p.get("detail")];print([p["done"] for p in e], [p["total"] for p in e], len(set(p["detail"] for p in e)))')" "[0, 1] [2, 2] 2"
ok "  the last word on the submodules is both done, nothing named" "$(printf '%s' "$TRACE" | "$PY" -c 'import sys,json;p=[p for p in json.load(sys.stdin) if p["phase"]=="Submodules"][-1];print(p["phase"],p["done"],p["total"],p["percent"],"detail" in p)')" "Submodules 2 2 100 False"

section "A submodule download that makes no headway is stopped; one beside it still arrives"
# A server that accepts the connection and never answers: git waits on it for
# ever, and the app must not — nor must it give up on a download that is
# merely slow, so the clock only runs while nothing is reported.
"$PY" - > "$SB/stuck-port" <<'PYSRV' &
import socket, sys, time
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(8)
print(s.getsockname()[1], flush=True)
while True:
    c, _ = s.accept(); time.sleep(3600)
PYSRV
STUCK_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/stuck-port" ] && break; sleep 0.5; done
PORT="$(cat "$SB/stuck-port")"
git config --global protocol.git.allow always          # the sandbox's own config: git:// is refused for submodules otherwise
git init -q "$SB/super-stuck"; printf 'r\n' > "$SB/super-stuck/README.md"; git -C "$SB/super-stuck" add -A >/dev/null; git -C "$SB/super-stuck" commit -qm init
( cd "$SB/super-stuck" && git submodule add -q "$SB/mapped.git" mapped 2>/dev/null )
git -C "$SB/super-stuck" config -f .gitmodules submodule.stuck.path stuck; git -C "$SB/super-stuck" config -f .gitmodules "submodule.stuck.url" "git://127.0.0.1:$PORT/stuck.git"
git -C "$SB/super-stuck" update-index --add --cacheinfo "160000,$(git -C "$SB/mapped.git" rev-parse HEAD),stuck"
git -C "$SB/super-stuck" add .gitmodules; git -C "$SB/super-stuck" commit -qm "two submodules, one of them stuck"
T0=$(date +%s); OUT=$(GITSWITCH_STALL_SECS=3 probe clone "" "$SB/super-stuck,$SB/clones,stuckclone,submodules"); T1=$(date +%s)
ok "the clone comes back with the good submodule, and the stuck one named" "$(printf '%s' "$OUT" | jqf submodules.downloaded):$(printf '%s' "$OUT" | jqf submodules.failed_paths)" "1:[\"stuck\"]"
ok "  stopped for making no headway, not for taking long" "$(printf '%s' "$OUT" | jqf submodules.error)" "made no progress for 3s and was stopped"
ok "  within seconds, not after the old three-minute limit" "$([ $((T1-T0)) -lt 60 ] && echo quick || echo "slow: $((T1-T0))s")" "quick"
ok "  and the page's line named it while it hung" "$(probe_trace | "$PY" -c 'import sys,json;e=[p for p in json.load(sys.stdin) if p["phase"]=="Submodules" and p.get("detail")=="stuck"];print(len(e)>0)')" "True"
ok "  the clone is still a repository the app can read" "$(probe status "$SB/clones/stuckclone" | jqf branch):$(git -C "$SB/clones/stuckclone" status --porcelain >/dev/null 2>&1 && echo readable || echo broken)" "main:readable"
ok "  and nothing half-deleted is left for the stuck one" "$([ -e "$SB/clones/stuckclone/.git/modules/stuck" ] && echo leftover || echo clean)" "clean"
kill "$STUCK_PID" 2>/dev/null; git config --global --unset protocol.git.allow
# A download that is quiet because it is checking out (a smudge filter, say) is not a stall.
git config --global filter.slow.smudge 'sleep 2; cat'; git config --global filter.slow.clean cat
git init -q --bare "$SB/slow.git"; git clone -q "$SB/slow.git" "$SB/author-slow" 2>/dev/null
printf '*.f filter=slow\n' > "$SB/author-slow/.gitattributes"; for i in 1 2 3; do printf '%s\n' "$i" > "$SB/author-slow/$i.f"; done
git -C "$SB/author-slow" add -A >/dev/null; git -C "$SB/author-slow" commit -qm "slow files"; git -C "$SB/author-slow" push -q origin main 2>/dev/null
git init -q "$SB/super-slow"; printf 'r\n' > "$SB/super-slow/README.md"; git -C "$SB/super-slow" add -A >/dev/null; git -C "$SB/super-slow" commit -qm init
( cd "$SB/super-slow" && git submodule add -q "$SB/slow.git" slow 2>/dev/null ); git -C "$SB/super-slow" commit -qm "a slow submodule"
OUT=$(GITSWITCH_STALL_SECS=3 probe clone "" "$SB/super-slow,$SB/clones,slowclone,submodules")
ok "a six-second silent checkout is not stopped by a three-second stall window: the download had finished" "$(printf '%s' "$OUT" | jqf submodules.downloaded):$(printf '%s' "$OUT" | jqf submodules.failed_paths):$(ls "$SB/clones/slowclone/slow" | grep -c '\.f$')" "1:[]:3"
git config --global --unset filter.slow.smudge; git config --global --unset filter.slow.clean

section "A submodule path with a space and a non-ASCII character"
git init -q --bare "$SB/uml.git"
git clone -q "$SB/uml.git" "$SB/uml_work" 2>/dev/null
cd "$SB/uml_work"; echo "uml v1" > file.txt; git add -A >/dev/null; git commit -qm "uml initial"; git push -q origin main 2>/dev/null
cd "$SB/super"
git submodule add -q "$SB/uml.git" "vendor/süb mod" 2>/dev/null
git add -A >/dev/null; git commit -qm "add a submodule with an awkward path"
ok "without -z, git C-quotes the path (the trap)" "$(git ls-files -s | grep -c 'vendor/s\\303\\274b mod' || true)" "1"
ok "  with -z it is the raw path" "$(git ls-files -s -z | tr '\0' '\n' | PYTHONIOENCODING=utf-8 python3 -c 'import sys;print("vendor/süb mod" in sys.stdin.read())')" "True"
OUT=$(probe submodules_list "$SB/super")
ok "the app lists it under its real path" "$(printf '%s' "$OUT" | PYTHONIOENCODING=utf-8 python3 -c 'import sys,json;print("vendor/süb mod" in [x["path"] for x in json.load(sys.stdin)])')" "True"
ok "  as initialised, mapped and clean" "$(printf '%s' "$OUT" | PYTHONIOENCODING=utf-8 python3 -c 'import sys,json;m=[x for x in json.load(sys.stdin) if x["path"]=="vendor/süb mod"][0];print(m["initialised"],m["listed"],m["state"])')" "True True clean"
OUT=$(probe sub_statuses "$SB/super")
ok "  and it gets a section, keyed by the same bytes as the parent's row" "$(printf '%s' "$OUT" | PYTHONIOENCODING=utf-8 python3 -c 'import sys,json;print("vendor/süb mod" in [x["path"] for x in json.load(sys.stdin)])')" "True"
echo "inside" >> "$SB/super/vendor/süb mod/file.txt"
OUT=$(probe status "$SB/super")
ok "  the parent's gitlink row uses those bytes" "$(printf '%s' "$OUT" | PYTHONIOENCODING=utf-8 python3 -c 'import sys,json;print([e["sub_tracked_changes"] for e in json.load(sys.stdin)["entries"] if e["path"]=="vendor/süb mod"])')" "[True]"
git -C "$SB/super/vendor/süb mod" checkout -q -- file.txt

section "A populated gitlink with no .gitmodules at all still counts as a submodule"
git init -q "$SB/embed"; cd "$SB/embed"; echo root > README.md; git add -A >/dev/null; git commit -qm "root"
git clone -q "$SB/orphan.git" "$SB/embed/nested" 2>/dev/null
# `git add` of a nested repository records a gitlink and writes no .gitmodules.
git add nested 2>/dev/null; git commit -qm "embed a repository"
ok "the fixture: a gitlink, no .gitmodules, a clean tree" "$(git ls-files -s nested | cut -c1-6):$([ -e .gitmodules ] && echo present || echo none):$(git status --porcelain | wc -l | tr -d ' ')" "160000:none:0"
OUT=$(probe status "$SB/embed")
ok "status says there are submodules" "$(printf '%s' "$OUT" | jqf has_submodules)" "True"
OUT=$(probe sub_statuses "$SB/embed")
ok "  and the section list has it" "$(printf '%s' "$OUT" | python3 -c 'import sys,json;print([x["path"] for x in json.load(sys.stdin)])')" "['nested']"
ok "  flagged as having no .gitmodules entry" "$(printf '%s' "$OUT" | sub nested listed)" "false"
ok "  with its own branch" "$(printf '%s' "$OUT" | sub nested status | jqf branch)" "main"

verify_result
