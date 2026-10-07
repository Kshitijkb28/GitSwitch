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

ok "  and downloading from the Changes page reports progress under the repository's path, for the page's strip" "$(printf '%s' "$TRACE_INIT" | "$PY" -c 'import sys,json;t=json.load(sys.stdin);print(sorted(set(p["phase"] for p in t)), t[0]["done"] if t else None)')" "['Submodules'] 0"

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

section "A push checks that the submodule commits it advertises are on a remote (what git push --recurse-submodules=check does)"
PC="$SB/pushcheck"; mkdir -p "$PC"
git init -q --bare "$PC/super.git"; git init -q --bare "$PC/lib.git"
git clone -q "$PC/lib.git" "$PC/author-lib" 2>/dev/null; printf 'lib v1\n' > "$PC/author-lib/lib.txt"; git -C "$PC/author-lib" add -A >/dev/null; git -C "$PC/author-lib" commit -qm "lib: initial"; git -C "$PC/author-lib" push -q origin main 2>/dev/null
git clone -q "$PC/super.git" "$PC/author" 2>/dev/null; printf 'readme\n' > "$PC/author/README.md"; git -C "$PC/author" add -A >/dev/null; git -C "$PC/author" commit -qm "super: initial"
( cd "$PC/author" && git submodule add -q "$PC/lib.git" lib 2>/dev/null ); git -C "$PC/author" commit -qam "super: add lib"; git -C "$PC/author" push -q origin main 2>/dev/null
git clone -q --recurse-submodules "$PC/super.git" "$PC/work" 2>/dev/null; git -C "$PC/work/lib" checkout -q main
W="$PC/work"
printf 'lib v2\n' > "$W/lib/lib.txt"; git -C "$W/lib" commit -qam "lib: v2"
git -C "$W" add lib >/dev/null; git -C "$W" commit -qm "super: bump lib to v2"
spush() { pinvoke changes_push "{\"repoPath\":\"$1\",\"setUpstream\":false,\"allowUnpushedSubmodules\":$2}"; }
row() { probe submodules_list "$1" | "$PY" -c 'import sys,json;s=json.load(sys.stdin)[0];print(s["recorded_on_remote"], s["own_unpushed"], s["own_branch"])'; }
ok "the submodule row knows its recorded commit is on no remote yet, and counts the commit" "$(row "$W")" "False 1 main"
OUT=$(spush "$W" false)
ok "pushing the superproject is refused, naming the submodule and its commit" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["refusal"]["code"], [(s["path"],s["state"]) for s in v["unpushed_submodules"]])')" "False submodules-unpushed [('lib', 'not-on-remote')]"
ok "  in words that say what would go wrong" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["refusal"]["message"])')" "lib records $(git -C "$W/lib" rev-parse --short=7 HEAD), which none of lib's remote-tracking branches has — as far as this checkout knows. Anyone fetching this push could not check it out. Push lib first (or fetch it, if that commit was pushed from elsewhere), or push anyway."
ok "  and nothing was pushed" "$(git -C "$PC/super.git" rev-parse main)" "$(git -C "$W" rev-parse HEAD~1)"
OUT=$(spush "$W/lib" false)
ok "pushing the submodule first works as any push" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["headline"])')" "True Pushed to origin."
ok "  the row now says the recorded commit is on its remote" "$(row "$W")" "True 0 main"
OUT=$(spush "$W" false)
ok "  and the superproject's push goes through, with nothing left to say" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["headline"], v.get("unpushed_submodules"))')" "True Pushed to origin. None"
printf 'lib v3\n' > "$W/lib/lib.txt"; git -C "$W/lib" commit -qam "lib: v3"; git -C "$W" add lib >/dev/null; git -C "$W" commit -qm "super: bump lib to v3"
OUT=$(spush "$W" true)
ok "push anyway pushes, and says what it leaves behind" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], [(s["path"],s["state"]) for s in v["unpushed_submodules"]], "Pushed anyway: lib records" in v["detail"])')" "True [('lib', 'not-on-remote')] True"
ok "  the remote has the superproject commit" "$(git -C "$PC/super.git" rev-parse main)" "$(git -C "$W" rev-parse HEAD)"
# A clone that never populated the submodule cannot be checked: said, not refused.
git clone -q "$PC/super.git" "$PC/work2" 2>/dev/null
git -C "$PC/work2" update-index --cacheinfo "160000,$(git -C "$PC/author-lib" rev-parse HEAD),lib" && git -C "$PC/work2" commit -qm "super: point lib at v1 again"
OUT=$(spush "$PC/work2" false)
ok "a pointer change in a never-populated submodule is pushed, with the check named as impossible" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], [(s["path"],s["state"]) for s in v["unpushed_submodules"]], "not initialised here" in v["detail"])')" "True [('lib', 'not-populated')] True"

section "A detached submodule with its own commits can start a branch where it stands"
git -C "$W/lib" checkout -q --detach HEAD
printf 'lib v4\n' > "$W/lib/lib.txt"; git -C "$W/lib" commit -qam "lib: v4 while detached"
ok "the row shows it detached, with its unpushed commits counted" "$(row "$W")" "False 2 None"
D=$(git -C "$W/lib" rev-parse HEAD)
OUT=$(pinvoke changes_create_branch "{\"repoPath\":\"$W/lib\",\"name\":\"fix/pointer\",\"from\":null,\"switchTo\":true}")
ok "a branch is created at the checkout and checked out, the commit kept" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"])'):$(git -C "$W/lib" symbolic-ref --short HEAD):$(git -C "$W/lib" rev-parse HEAD):$(git -C "$W/lib" log --format=%s -1)" "True:fix/pointer:$D:lib: v4 while detached"
ok "  the row shows the branch now" "$(row "$W")" "False 2 fix/pointer"
OUT=$(pinvoke changes_push "{\"repoPath\":\"$W/lib\",\"setUpstream\":true,\"allowUnpushedSubmodules\":false}")
ok "  and it can be published from there" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"])'):$(git -C "$PC/lib.git" rev-parse fix/pointer)" "True:$D"
ok "  after which nothing of it is unpushed — and the recorded commit is on a remote too, through the new branch" "$(row "$W")" "True 0 fix/pointer"

section "A submodule added upstream after the clone can be downloaded from the app — after a Pull, by Update, and inside a submodule"
NS="$SB/newsub"; mkdir -p "$NS/clones"
git init -q --bare "$NS/super.git"; for s in A B inner; do git init -q --bare "$NS/$s.git"; git clone -q "$NS/$s.git" "$NS/author-$s" 2>/dev/null; printf '%s v1\n' "$s" > "$NS/author-$s/$s.txt"; git -C "$NS/author-$s" add -A >/dev/null; git -C "$NS/author-$s" commit -qm "$s: initial"; git -C "$NS/author-$s" push -q origin main 2>/dev/null; done
git clone -q "$NS/super.git" "$NS/author" 2>/dev/null; printf 'readme\n' > "$NS/author/README.md"; git -C "$NS/author" add -A >/dev/null; git -C "$NS/author" commit -qm "super: initial"
( cd "$NS/author" && git submodule add -q "$NS/A.git" A 2>/dev/null ); git -C "$NS/author" commit -qam "super: add A" >/dev/null; git -C "$NS/author" push -q origin main 2>/dev/null
OUT=$(probe clone "" "$NS/super.git,$NS/clones,work,submodules"); W="$NS/clones/work"
ok "the fixture: cloned by the app with its one submodule" "$(printf '%s' "$OUT" | jqf submodules.downloaded):$([ -e "$W/A/.git" ] && echo yes || echo no)" "1:yes"
# Upstream adds B.
( cd "$NS/author" && git submodule add -q "$NS/B.git" B 2>/dev/null ); git -C "$NS/author" commit -qam "super: add B" >/dev/null; git -C "$NS/author" push -q origin main 2>/dev/null
OUT=$(pinvoke changes_pull "{\"repoPath\":\"$W\",\"mode\":\"ff-only\",\"withLfs\":false,\"autostash\":false}")
ok "after a Pull the new submodule is listed as not downloaded, with the download offered" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["ok"])'):$(probe submodules_list "$W" | "$PY" -c 'import sys,json;s=[x for x in json.load(sys.stdin) if x["path"]=="B"][0];print(s["initialised"], s["listed"], s["state"], s["summary"])')" "True:False True not-initialised not downloaded yet — Download fetches it (git submodule update --init)"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$W\",\"paths\":[\"B\"]}")
ok "  Download fetches it at the recorded commit" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["ok"])'):$(git -C "$W/B" rev-parse HEAD 2>/dev/null)" "True:$(git -C "$W" rev-parse HEAD:B)"
# Through Update: upstream adds C (declared in .gitmodules) and a gitlink D without an entry.
git init -q --bare "$NS/C.git"; git clone -q "$NS/C.git" "$NS/author-C" 2>/dev/null; printf 'C v1\n' > "$NS/author-C/C.txt"; git -C "$NS/author-C" add -A >/dev/null; git -C "$NS/author-C" commit -qm "C: initial"; git -C "$NS/author-C" push -q origin main 2>/dev/null
# `submodule add` stages C and .gitmodules; D is a gitlink staged by hand with no folder, so no `-a` (it would stage D's "deletion").
( cd "$NS/author" && git submodule add -q "$NS/C.git" C 2>/dev/null ); git -C "$NS/author" update-index --add --cacheinfo "160000,$(git -C "$NS/author-C" rev-parse HEAD),D"; git -C "$NS/author" commit -qm "super: add C, and D without an entry" >/dev/null; git -C "$NS/author" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W")
ok "the preview names the submodule upstream added and says it will be downloaded, and the one nothing can fetch" "$(printf '%s' "$OUT" | jqf superproject.new_submodules):$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;d=json.load(sys.stdin);print([n for n in d["superproject"]["notes"] if "upstream added" in n])')" "[\"C\"]:['upstream added 1 submodule: C — downloaded after the rebase, at the commit it records (git submodule update --init).', 'upstream added 1 submodule without an entry in .gitmodules: D — git cannot fetch it, so it stays empty.']"
OUT=$(probe sync_run "$W")
ok "the update downloads it at the recorded commit and says so" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["sync"];print([(s["path"],s["action"],s["ok"]) for s in d["submodules"] if s["path"]=="C"])'):$(git -C "$W/C" rev-parse HEAD 2>/dev/null)" "True:[('C', 'downloaded', True)]:$(git -C "$W" rev-parse HEAD:C)"
ok "  the checks still pass, and the unmapped one stays empty" "$(printf '%s' "$OUT" | jqf sync.verified.submodules_aligned):$(ls -A "$W/D" 2>/dev/null | wc -l | tr -d ' ')" "True:0"
# Inside a submodule: upstream's A gains a submodule of its own; the app downloads it with A as the scope.
( cd "$NS/author-A" && git submodule add -q "$NS/inner.git" inner 2>/dev/null ); git -C "$NS/author-A" commit -qam "A: add inner" >/dev/null; git -C "$NS/author-A" push -q origin main 2>/dev/null
git -C "$NS/author/A" pull -q --ff-only origin main 2>/dev/null; git -C "$NS/author" add A >/dev/null; git -C "$NS/author" commit -qm "super: bump A (it gained inner)" >/dev/null; git -C "$NS/author" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W" >/dev/null; probe sync_run "$W")
ok "after the update, A's own new submodule is listed with A as the scope, not downloaded yet" "$(printf '%s' "$OUT" | jqf ok):$(probe submodules_list "$W/A" | "$PY" -c 'import sys,json;print([(s["path"],s["initialised"],s["listed"]) for s in json.load(sys.stdin)])')" "True:[('inner', False, True)]"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$W/A\",\"paths\":[\"inner\"]}")
ok "  and Download works there too" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["ok"])'):$(git -C "$W/A/inner" rev-parse HEAD 2>/dev/null)" "True:$(git -C "$W/A" rev-parse HEAD:inner)"

section "The push check sees what git's own check sees"
# A fresh superproject + submodule pair with the submodule at a given path; work/ is a recursive clone on main.
pair() { local D="$1" SP="$2"; mkdir -p "$D"; git init -q --bare "$D/super.git"; git init -q --bare "$D/lib.git"
  git clone -q "$D/lib.git" "$D/author-lib" 2>/dev/null; printf 'lib v1\n' > "$D/author-lib/lib.txt"; git -C "$D/author-lib" add -A >/dev/null; git -C "$D/author-lib" commit -qm "lib: initial"; git -C "$D/author-lib" push -q origin main 2>/dev/null
  git clone -q "$D/super.git" "$D/author" 2>/dev/null; printf 'readme\n' > "$D/author/README.md"; git -C "$D/author" add -A >/dev/null; git -C "$D/author" commit -qm "super: initial"
  ( cd "$D/author" && git submodule add -q "$D/lib.git" "$SP" 2>/dev/null ); git -C "$D/author" commit -qam "super: add $SP" >/dev/null; git -C "$D/author" push -q origin main 2>/dev/null
  git clone -q --recurse-submodules "$D/super.git" "$D/work" 2>/dev/null; git -C "$D/work/$SP" checkout -q main; }
bump() { local W="$1" SP="$2" V="$3"; printf 'lib %s\n' "$V" > "$W/$SP/lib.txt"; git -C "$W/$SP" commit -qam "lib: $V"; git -C "$W" add -- "$SP" >/dev/null; git -C "$W" commit -qm "super: bump $SP to $V"; }
spush2() { pinvoke changes_push "{\"repoPath\":\"$1\",\"setUpstream\":false,\"allowUnpushedSubmodules\":false}" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], (v.get("refusal") or {}).get("code"), [(s["path"],s["recorded_short"],s["state"],s["branch"]) for s in v.get("unpushed_submodules") or []])'; }
# (a) a path with non-ASCII bytes: git quotes it in plain --name-only output; the check reads it with -z.
pair "$SB/pcq" "lïb"; bump "$SB/pcq/work" "lïb" v2
ok "a submodule at a non-ASCII path is checked like any other" "$(spush2 "$SB/pcq/work")" "False submodules-unpushed [('lïb', '$(git -C "$SB/pcq/work/lïb" rev-parse --short=7 HEAD)', 'not-on-remote', 'main')]"
# (b) a pointer set by an intermediate commit of the push and moved back in HEAD: a clone may check that commit out.
pair "$SB/pci" lib; bump "$SB/pci/work" lib v2; git -C "$SB/pci/work/lib" push -q origin main 2>/dev/null
bump "$SB/pci/work" lib v3; V3=$(git -C "$SB/pci/work/lib" rev-parse --short=7 HEAD)
git -C "$SB/pci/work/lib" checkout -q "$(git -C "$SB/pci/work/lib" rev-parse HEAD~1)" 2>/dev/null; git -C "$SB/pci/work" add lib >/dev/null; git -C "$SB/pci/work" commit -qm "super: back to v2"
ok "a pointer only an intermediate pushed commit sets is checked too, though HEAD's own is on the remote" "$(spush2 "$SB/pci/work")" "False submodules-unpushed [('lib', '$V3', 'not-on-remote', None)]"
# (c) a gitlink removed from the index but still recorded by HEAD, which is what gets pushed.
pair "$SB/pcr" lib; bump "$SB/pcr/work" lib v2; git -C "$SB/pcr/work" rm -q --cached lib
ok "a gitlink staged for removal is still checked: HEAD records it" "$(spush2 "$SB/pcr/work")" "False submodules-unpushed [('lib', '$(git -C "$SB/pcr/work/lib" rev-parse --short=7 HEAD)', 'not-on-remote', 'main')]"
git -C "$SB/pcr/work" reset -q -- lib
# (d) the commit is on a remote — but not the one a clone fetches from, the address .gitmodules declares.
pair "$SB/pcf" lib; git init -q --bare "$SB/pcf/fork.git"; git -C "$SB/pcf/work/lib" remote add fork "$SB/pcf/fork.git"; bump "$SB/pcf/work" lib v2; git -C "$SB/pcf/work/lib" push -q fork main 2>/dev/null
ok "a commit pushed only to another remote of the submodule does not count: a clone fetches from the declared address" "$(spush2 "$SB/pcf/work")" "False submodules-unpushed [('lib', '$(git -C "$SB/pcf/work/lib" rev-parse --short=7 HEAD)', 'not-on-remote', 'main')]"
ok "  and the row says so too" "$(probe submodules_list "$SB/pcf/work" | "$PY" -c 'import sys,json;s=json.load(sys.stdin)[0];print(s["recorded_on_remote"], s["own_unpushed"])')" "False 1"
git -C "$SB/pcf/work/lib" push -q origin main 2>/dev/null
ok "  pushed to the declared remote, it counts" "$(spush2 "$SB/pcf/work")" "True None []"
# (e) a detached submodule: the refusal asks for a branch there first, since a push from it needs one.
pair "$SB/pcd" lib; git -C "$SB/pcd/work/lib" checkout -q --detach; bump "$SB/pcd/work" lib v2
# (the call on its own line: macOS bash 3.2 mangles \" inside "$( )")
OUT=$(pinvoke changes_push "{\"repoPath\":\"$SB/pcd/work\",\"setUpstream\":false,\"allowUnpushedSubmodules\":false}")
ok "for a detached submodule the refusal asks for a branch there first" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["refusal"]["message"])')" "Push lib first (it is detached — start a branch there first), or push anyway."

section "A download that was interrupted is not 'populated': it is finished by Download"
IC="$SB/incomplete"; mkdir -p "$IC"
git init -q --bare "$IC/super.git"; git init -q --bare "$IC/lib.git"
git clone -q "$IC/lib.git" "$IC/author-lib" 2>/dev/null; printf 'lib v1\n' > "$IC/author-lib/lib.txt"; git -C "$IC/author-lib" add -A >/dev/null; git -C "$IC/author-lib" commit -qm "lib: initial"; git -C "$IC/author-lib" push -q origin main 2>/dev/null
git clone -q "$IC/super.git" "$IC/author" 2>/dev/null; printf 'readme\n' > "$IC/author/README.md"; git -C "$IC/author" add -A >/dev/null; git -C "$IC/author" commit -qm "super: initial"
( cd "$IC/author" && git submodule add -q "$IC/lib.git" lib 2>/dev/null ); git -C "$IC/author" commit -qam "super: add lib" >/dev/null; git -C "$IC/author" push -q origin main 2>/dev/null
git clone -q "$IC/super.git" "$IC/work" 2>/dev/null; W="$IC/work"
# What a crash or a stall mid-download leaves: the gitfile and a module directory without a commit.
# (`--separate-git-dir` makes exactly what `git submodule update` makes first: the gitfile in lib/, the git dir under .git/modules/.)
mkdir -p "$W/.git/modules"; git init -q --separate-git-dir "$W/.git/modules/lib" "$W/lib" 2>/dev/null
ok "the repository's own status still reads with such a submodule in it" "$(probe status "$W" | jqf branch)" "main"
OUT=$(probe submodules_list "$W")
ok "the row says the download was interrupted, and is not counted as populated" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;s=json.load(sys.stdin)[0];print(s["initialised"], s["actual"], s["state"], s["summary"])')" "True None incomplete its download was interrupted — nothing is checked out; Download finishes it (git submodule update --init)"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$W\",\"paths\":[\"lib\"]}")
ok "Download clears the leftovers and finishes it, saying so" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["submodules"]["downloaded"], v["submodules"]["cleared"], "was cleared before downloading again" in v["detail"])')" "True 1 ['lib'] True"
ok "  and lib is checked out at the recorded commit" "$(git -C "$W/lib" rev-parse HEAD 2>/dev/null):$(probe submodules_list "$W" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)[0]["state"])')" "$(git -C "$W" rev-parse HEAD:lib):clean"
# A module directory that does hold commits is never removed: a submodule whose HEAD points nowhere is finished by a checkout, its commits kept.
git -C "$W/lib" symbolic-ref HEAD refs/heads/nowhere
OUT=$(probe submodules_list "$W")
ok "a submodule with commits but nothing checked out is reported as incomplete too" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;s=json.load(sys.stdin)[0];print(s["state"])')" "incomplete"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$W\",\"paths\":[\"lib\"]}")
ok "  Download does not throw its commits away" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["submodules"]["cleared"])'):$([ "$(git -C "$W/.git/modules/lib" rev-list --all --count)" -gt 0 ] && echo kept || echo GONE):$(git -C "$W/lib" rev-parse HEAD 2>/dev/null)" "True []:kept:$(git -C "$W" rev-parse HEAD:lib)"
# An interrupted transfer: HEAD gone, only a temporary pack — nothing git can read, nothing to keep.
rm -f "$W/.git/modules/lib/HEAD"; find "$W/.git/modules/lib/refs" -type f -delete; rm -f "$W/.git/modules/lib/packed-refs"; rm -rf "$W/.git/modules/lib/objects"; mkdir -p "$W/.git/modules/lib/objects/pack"; printf 'partial' > "$W/.git/modules/lib/objects/pack/tmp_pack_abc"; find "$W/lib" -mindepth 1 ! -name .git -delete
ok "  a module directory git cannot even open, with only a temporary pack, is seen as incomplete" "$(probe submodules_list "$W" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)[0]["state"])')" "incomplete"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$W\",\"paths\":[\"lib\"]}")
ok "  and Download clears it and downloads afresh" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["submodules"]["cleared"])'):$(git -C "$W/lib" rev-parse HEAD 2>/dev/null)" "True ['lib']:$(git -C "$W" rev-parse HEAD:lib)"
# The harsher leftover: a bare module directory, which git cannot even run status in. The page survives it, and Download clears it.
git clone -q "$IC/super.git" "$IC/work2" 2>/dev/null; W2="$IC/work2"
mkdir -p "$W2/.git/modules/lib"; git init -q --bare "$W2/.git/modules/lib" 2>/dev/null; printf 'gitdir: ../.git/modules/lib\n' > "$W2/lib/.git"
ok "a bare leftover does not take the repository's status down" "$(probe status "$W2" | jqf branch):$(probe submodules_list "$W2" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)[0]["state"])')" "main:incomplete"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$W2\",\"paths\":[\"lib\"]}")
ok "  and Download clears it and finishes the download" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["submodules"]["cleared"])'):$(git -C "$W2/lib" rev-parse HEAD 2>/dev/null)" "True ['lib']:$(git -C "$W2" rev-parse HEAD:lib)"

section "A download that takes a while says how long is left"
# A submodule big enough to watch, behind a remote that sends it slowly: the
# pack goes through a hook that trickles it out. (git honours that hook from
# the sandbox's own global config only, and only on the pack protocol — hence
# the file:// address; a plain path is hard-linked and never downloads.)
git init -q --bare "$SB/big.git"; git clone -q "$SB/big.git" "$SB/author-big" 2>/dev/null
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24; do head -c 131072 /dev/urandom > "$SB/author-big/blob$i.bin"; done
git -C "$SB/author-big" add -A >/dev/null; git -C "$SB/author-big" commit -qm "about 3 MiB that will not compress"; git -C "$SB/author-big" push -q origin main 2>/dev/null
cat > "$SB/trickle.sh" <<'TRICKLE'
#!/bin/sh
# Run the real pack-objects and pass its output on a little at a time.
"$@" | perl -e '$|=1; binmode STDIN; binmode STDOUT; while (read(STDIN, $b, 32768)) { print $b; select(undef, undef, undef, 0.1); }'
TRICKLE
chmod +x "$SB/trickle.sh"
git config --global uploadpack.packObjectsHook "$SB/trickle.sh"
git init -q "$SB/super-big"; printf 'r\n' > "$SB/super-big/README.md"; git -C "$SB/super-big" add -A >/dev/null; git -C "$SB/super-big" commit -qm init
( cd "$SB/super-big" && git submodule add -q "file://$SB/big.git" big 2>/dev/null ); git -C "$SB/super-big" commit -qm "a big submodule"
OUT=$(probe clone "" "$SB/super-big,$SB/clones,bigclone,submodules")
git config --global --unset uploadpack.packObjectsHook
TRACE_BIG=$(probe_trace)
ok "the slow submodule arrives all the same" "$(printf '%s' "$OUT" | jqf submodules.downloaded):$(ls "$SB/clones/bigclone/big" | wc -l | tr -d ' ')" "1:24"
ok "  while it came in, its line said how much had arrived and how fast" "$(printf '%s' "$TRACE_BIG" | "$PY" -c 'import sys,json;e=[p for p in json.load(sys.stdin) if p["phase"]=="Submodules" and p.get("detail")=="big" and p.get("bytes") and p.get("rate")];print(len(e)>0, e[-1]["bytes"] if e else "", e[-1]["rate"] if e else "")')" "True"
ok "  and how long was left, from the share of objects that had arrived" "$(printf '%s' "$TRACE_BIG" | "$PY" -c 'import sys,json;e=[p["eta_secs"] for p in json.load(sys.stdin) if p["phase"]=="Submodules" and p.get("detail")=="big" and isinstance(p.get("eta_secs"),int)];print(len(e)>0 and all(0<=x<600 for x in e), e[:6])')" "True"
ok "  the time left goes down as it arrives" "$(printf '%s' "$TRACE_BIG" | "$PY" -c 'import sys,json;e=[p["eta_secs"] for p in json.load(sys.stdin) if p["phase"]=="Submodules" and p.get("detail")=="big" and isinstance(p.get("eta_secs"),int)];print(len(e)>=2 and e[-1]<=e[0], e[:1], e[-1:])')" "True"
ok "  no size is claimed for a host that was never asked" "$(printf '%s' "$TRACE_BIG" | "$PY" -c 'import sys,json;print(any("estimate_kb" in p for p in json.load(sys.stdin)))')" "False"

section "A downloaded submodule that has to fetch its new commit shows the download, and a slow one is not stopped"
# Downloaded before; the superproject now records a commit the submodule does
# not have. git's own `submodule update` fetches it without a word of
# progress — nothing to show, and a long fetch looked like a stall.
git init -q --bare "$SB/grow.git"; git clone -q "file://$SB/grow.git" "$SB/author-grow" 2>/dev/null
printf 'v1\n' > "$SB/author-grow/v.txt"; git -C "$SB/author-grow" add -A >/dev/null; git -C "$SB/author-grow" commit -qm "grow: v1"; git -C "$SB/author-grow" push -q origin main 2>/dev/null
git init -q --bare "$SB/super-grow.git"; git clone -q "$SB/super-grow.git" "$SB/author-super-grow" 2>/dev/null
printf 'r\n' > "$SB/author-super-grow/README.md"; git -C "$SB/author-super-grow" add -A >/dev/null; git -C "$SB/author-super-grow" commit -qm init
( cd "$SB/author-super-grow" && git submodule add -q "file://$SB/grow.git" grow 2>/dev/null ); git -C "$SB/author-super-grow" commit -qm "add grow"; git -C "$SB/author-super-grow" push -q origin main 2>/dev/null
git clone -q --recurse-submodules "$SB/super-grow.git" "$SB/grow-work" 2>/dev/null
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24; do head -c 131072 /dev/urandom > "$SB/author-grow/g$i.bin"; done
git -C "$SB/author-grow" add -A >/dev/null; git -C "$SB/author-grow" commit -qm "grow: about 3 MiB more"; git -C "$SB/author-grow" push -q origin main 2>/dev/null
git -C "$SB/author-super-grow/grow" pull -q --ff-only origin main 2>/dev/null; git -C "$SB/author-super-grow" add grow; git -C "$SB/author-super-grow" commit -qm "grow: bump"; git -C "$SB/author-super-grow" push -q origin main 2>/dev/null
# The superproject alone: git's own pull would fetch the submodule's commit on
# the quiet (`fetch.recurseSubmodules=on-demand`), which is what the app's
# Pull turns off and what a plain `git pull --no-recurse-submodules` does.
git -C "$SB/grow-work" pull -q --ff-only --no-recurse-submodules origin main 2>/dev/null
WANT=$(git -C "$SB/grow-work" ls-files -s -- grow | awk '{print $2}')
ok "(the commit the superproject now records is not in the downloaded submodule yet)" "$(git -C "$SB/grow-work/grow" cat-file -e "$WANT^{commit}" 2>/dev/null && echo present || echo absent)" "absent"
git config --global uploadpack.packObjectsHook "$SB/trickle.sh"
T0=$(date +%s); OUT=$(GITSWITCH_STALL_SECS=3 pinvoke changes_submodule_update "{\"repoPath\":\"$SB/grow-work\",\"paths\":[\"grow\"]}"); T1=$(date +%s)
TRACE_GROW=$(probe_trace)
git config --global --unset uploadpack.packObjectsHook
ok "a submodule that had to fetch 3 MiB slowly reaches the recorded commit — not stopped" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], v["submodules"]["downloaded"], v["submodules"]["failed_paths"])'):$(git -C "$SB/grow-work/grow" rev-parse HEAD)" "True 1 []:$WANT"
ok "  it really took longer than the silence allowed (3 s)" "$([ $((T1-T0)) -ge 5 ] && echo "slow enough" || echo "too quick to prove anything: $((T1-T0))s")" "slow enough"
ok "  while it fetched, its line said how much had arrived and how fast" "$(printf '%s' "$TRACE_GROW" | "$PY" -c 'import sys,json;e=[p for p in json.load(sys.stdin) if p["phase"]=="Submodules" and p.get("detail")=="grow" and p.get("bytes") and p.get("rate")];print(len(e)>0)')" "True"
ok "  and how long was left" "$(printf '%s' "$TRACE_GROW" | "$PY" -c 'import sys,json;print(any(isinstance(p.get("eta_secs"),int) for p in json.load(sys.stdin) if p.get("detail")=="grow"))')" "True"
ok "  with no size from a host — a fetch brings only what is new" "$(printf '%s' "$TRACE_GROW" | "$PY" -c 'import sys,json;print(any("estimate_kb" in p for p in json.load(sys.stdin)))')" "False"
ok "  and the folder is a clean checkout of it" "$(git -C "$SB/grow-work/grow" status --porcelain | wc -l | tr -d ' '):$(ls "$SB/grow-work/grow" | grep -c '^g')" "0:24"
OUT=$(PROBE_LEFT_FILE="$SB/left.json" PROBE_LEFT_FOR="$SB/grow-work" pinvoke changes_submodule_update "{\"repoPath\":\"$SB/grow-work\",\"paths\":[\"grow\"]}")
ok "  when a download is over its line is gone, so the next one does not open on it" "$(printf '%s' "$(probe_trace)" | "$PY" -c 'import sys,json;print(len(json.load(sys.stdin))>0)'):$(cat "$SB/left.json")" "True:null"

section "Submodules at dot-prefixed paths (.memory, .audit, a nested .seed) are cloned, listed and covered like any other"
for r in memory audit seed; do git init -q --bare "$SB/dot-$r.git"; git clone -q "$SB/dot-$r.git" "$SB/author-dot-$r" 2>/dev/null; printf '%s\n' "$r" > "$SB/author-dot-$r/$r.txt"; git -C "$SB/author-dot-$r" add -A >/dev/null; git -C "$SB/author-dot-$r" commit -qm "$r: initial"; git -C "$SB/author-dot-$r" push -q origin main 2>/dev/null; done
( cd "$SB/author-dot-audit" && git submodule add -q "$SB/dot-seed.git" .seed 2>/dev/null ); git -C "$SB/author-dot-audit" commit -qm "audit: add .seed"; git -C "$SB/author-dot-audit" push -q origin main 2>/dev/null
git init -q --bare "$SB/super-dots.git"; git clone -q "$SB/super-dots.git" "$SB/author-super-dots" 2>/dev/null
printf 'r\n' > "$SB/author-super-dots/README.md"; git -C "$SB/author-super-dots" add -A >/dev/null; git -C "$SB/author-super-dots" commit -qm init
( cd "$SB/author-super-dots" && git submodule add -q "$SB/dot-memory.git" .memory 2>/dev/null && git submodule add -q "$SB/dot-audit.git" .audit 2>/dev/null )
git -C "$SB/author-super-dots" commit -qm "add .memory and .audit"; git -C "$SB/author-super-dots" push -q origin main 2>/dev/null
OUT=$(probe clone "" "$SB/super-dots.git,$SB/clones,dotclone,submodules")
D="$SB/clones/dotclone"
ok "cloning with submodules downloads both" "$(printf '%s' "$OUT" | jqf submodules.downloaded):$(printf '%s' "$OUT" | jqf submodules.listed):$(printf '%s' "$OUT" | jqf submodules.failed_paths)" "2:2:[]"
ok "  and the nested one inside .audit" "$(cat "$D/.audit/.seed/seed.txt" 2>/dev/null || echo MISSING)" "seed"
ok "  the Submodules tab lists them as downloaded and up to date — not missing, not interrupted" "$(probe submodules_list "$D" | "$PY" -c 'import sys,json;print(sorted((s["path"], s["state"]) for s in json.load(sys.stdin)))')" "[('.audit', 'clean'), ('.memory', 'clean')]"
ok "  the repository's status is whole" "$(probe status "$D" | jqf branch):$(probe status "$D" | jqf has_submodules)" "main:True"
OUT=$(probe push_mode "$D" "guardrail")
ok "blocking pushes reaches all three, the nested one included" "$(pinvoke changes_push_coverage "{\"repoPath\":\"$D\"}" | "$PY" -c 'import sys,json;c=json.load(sys.stdin)["value"];print(sorted(s["path"] for s in c["submodules"] if s["covered"]), c["summary"])')" "['.audit', '.audit/.seed', '.memory'] Covers this repository and 3 submodules."
git -C "$D/.memory" push origin main >/dev/null 2>&1; RC=$?
ok "  a push from inside .memory is refused in a plain terminal" "$([ "$RC" -ne 0 ] && echo refused || echo PUSHED)" "refused"
OUT=$(probe push_mode "$D" "allowed")
ok "  and allowing it again frees them" "$(git -C "$D/.memory" config --local --get gitswitch.pushBlocked || echo unset):$(git -C "$D/.audit/.seed" config --local --get gitswitch.pushBlocked || echo unset)" "unset:unset"

section "A submodule's size is asked for only on its first full download"
# A fake API that logs every request and says each repository weighs 8 MiB.
# The submodules' addresses are GitHub's; the sandbox's own config sends the
# download itself to local copies (url.<local>.insteadOf).
"$PY" - "$SB/api.log" > "$SB/api-port" <<'PYAPI' &
import http.server, sys, json
log = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(log, "a") as f:
            f.write("%s token=%s\n" % (self.path, "yes" if self.headers.get("Authorization") else "no"))
        body = json.dumps({"size": 8192}).encode()
        self.send_response(200 if self.path.startswith("/repos/acme/") else 404)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
PYAPI
API_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/api-port" ] && break; sleep 0.5; done
API="http://127.0.0.1:$(cat "$SB/api-port")"
for r in widget shal; do
  git init -q --bare "$SB/$r.git"; git clone -q "file://$SB/$r.git" "$SB/author-$r" 2>/dev/null
  printf '%s\n' "$r" > "$SB/author-$r/$r.txt"; git -C "$SB/author-$r" add -A >/dev/null; git -C "$SB/author-$r" commit -qm "$r: v1"; git -C "$SB/author-$r" push -q origin main 2>/dev/null
  git config --global "url.file://$SB/$r.git.insteadOf" "https://github.com/acme/$r.git"
done
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24; do head -c 131072 /dev/urandom > "$SB/author-widget/w$i.bin"; done
git -C "$SB/author-widget" add -A >/dev/null; git -C "$SB/author-widget" commit -qm "widget: 3 MiB"; git -C "$SB/author-widget" push -q origin main 2>/dev/null
git init -q --bare "$SB/super-api.git"; git clone -q "$SB/super-api.git" "$SB/author-super-api" 2>/dev/null
printf 'r\n' > "$SB/author-super-api/README.md"; git -C "$SB/author-super-api" add -A >/dev/null; git -C "$SB/author-super-api" commit -qm init
( cd "$SB/author-super-api" && git submodule add -q "https://github.com/acme/widget.git" widget 2>/dev/null && git submodule add -q "https://github.com/acme/shal.git" shal 2>/dev/null )
git -C "$SB/author-super-api" config -f .gitmodules submodule.shal.shallow true
git -C "$SB/author-super-api" add -A >/dev/null; git -C "$SB/author-super-api" commit -qm "two submodules, one shallow"; git -C "$SB/author-super-api" push -q origin main 2>/dev/null
git clone -q "$SB/super-api.git" "$SB/api-work" 2>/dev/null
: > "$SB/api.log"
git config --global uploadpack.packObjectsHook "$SB/trickle.sh"
OUT=$(GITSWITCH_GITHUB_API="$API" pinvoke changes_submodule_update "{\"repoPath\":\"$SB/api-work\",\"paths\":[\"widget\"]}")
TRACE_API=$(probe_trace)
git config --global --unset uploadpack.packObjectsHook
ok "a first download asks the host once how big the submodule is" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["submodules"]["downloaded"])'):$(grep -c '^/repos/acme/widget ' "$SB/api.log")" "1:1"
ok "  and shows how much of that has arrived" "$(printf '%s' "$TRACE_API" | "$PY" -c 'import sys,json;print(any(p.get("estimate_kb")==8192 for p in json.load(sys.stdin) if p.get("detail")=="widget"))')" "True"
OUT=$(GITSWITCH_GITHUB_API="$API" pinvoke changes_submodule_update "{\"repoPath\":\"$SB/api-work\",\"paths\":[\"widget\"]}")
ok "updating a submodule that already has its commit asks nothing — nothing is downloaded" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["ok"])'):$(grep -c '^/repos/acme/widget ' "$SB/api.log")" "True:1"
OUT=$(GITSWITCH_GITHUB_API="$API" pinvoke changes_submodule_update "{\"repoPath\":\"$SB/api-work\",\"paths\":[\"shal\"]}")
ok "a shallow submodule is not asked about — it downloads one commit, not the history the host weighs" "$(cat "$SB/api-work/shal/shal.txt" 2>/dev/null || echo MISSING):$(grep -c '^/repos/acme/shal ' "$SB/api.log")" "shal:0"
{ kill "$API_PID" && wait "$API_PID"; } 2>/dev/null
for r in widget shal; do git config --global --unset "url.file://$SB/$r.git.insteadOf"; done

verify_result
