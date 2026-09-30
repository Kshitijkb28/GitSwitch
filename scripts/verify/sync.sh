#!/bin/bash
# Sync suite: "take the latest, keep my commits on top", submodules included.
# The fixture is the one the design was settled on (scripts/verify/sync-facts.md):
# a superproject with submodules A and B, an upstream developer who moves both,
# and a local clone with its own commits in A and in the superproject, plus
# uncommitted edits and untracked files. Local bare repos stand in for remotes.
set -u
source "$(dirname "$0")/lib.sh"
verify_init sync "Sync Tester" "sync@example.test"
SB="$(realdir "$SB")"

subjects() { git -C "$1" log --format=%s "$2" | tr '\n' '|'; }
head_of() { git -C "$1" rev-parse HEAD; }
on_branch() { git -C "$1" symbolic-ref -q --short HEAD 2>/dev/null || echo DETACHED; }
restore() { rm -rf "$SB/local"; cp -R "$SB/local.pristine" "$SB/local"; }

# --- fixture ------------------------------------------------------------------
for r in super A B orphan; do git init -q --bare "$SB/$r.git"; done
for r in A B orphan; do
  git clone -q "$SB/$r.git" "$SB/author/$r" 2>/dev/null
  printf '%s v1\nline2\nline3\n' "$r" > "$SB/author/$r/$r.txt"
  git -C "$SB/author/$r" add -A >/dev/null; git -C "$SB/author/$r" commit -qm "$r: initial"; git -C "$SB/author/$r" push -q origin main 2>/dev/null
done
git clone -q "$SB/super.git" "$SB/author/super" 2>/dev/null
cd "$SB/author/super"
printf 'shared line 1\nshared line 2\nshared line 3\n' > shared.txt; printf 'readme\n' > README.md
git add -A >/dev/null; git commit -qm "super: initial"
for s in A B orphan; do git submodule add -q "$SB/$s.git" "$s" 2>/dev/null; git config -f .gitmodules "submodule.$s.branch" main; git config -f .gitmodules "submodule.$s.update" merge; done
git add -A >/dev/null; git commit -qm "super: add submodules"
# An unmapped gitlink, as real superprojects have: keep the pointer, drop the mapping.
git config -f .gitmodules --remove-section submodule.orphan; git add .gitmodules; git commit -qm "super: drop orphan mapping"
git push -q origin main 2>/dev/null

# local: cloned BEFORE upstream's work, submodules on their branch
git clone -q --recurse-submodules "$SB/super.git" "$SB/local" 2>/dev/null
for s in A B; do git -C "$SB/local/$s" checkout -q main; done

# upstream: another developer moves A (2 commits), B (1 commit) and the superproject
git clone -q --recurse-submodules "$SB/super.git" "$SB/upstream" 2>/dev/null
for s in A B; do git -C "$SB/upstream/$s" checkout -q main; done
for i in 1 2; do printf 'upstream %s\n' "$i" >> "$SB/upstream/A/up$i.txt"; git -C "$SB/upstream/A" add -A >/dev/null; git -C "$SB/upstream/A" commit -qm "A: upstream $i"; done
git -C "$SB/upstream/A" push -q origin main 2>/dev/null
git -C "$SB/upstream" add A >/dev/null; git -C "$SB/upstream" commit -qm "super: bump A"
printf 'shared line 1\nshared line 2 (upstream)\nshared line 3\n' > "$SB/upstream/shared.txt"; printf 'up\n' > "$SB/upstream/upstream.txt"
git -C "$SB/upstream" add -A >/dev/null; git -C "$SB/upstream" commit -qm "super: upstream work"
printf 'b2\n' > "$SB/upstream/B/b2.txt"; git -C "$SB/upstream/B" add -A >/dev/null; git -C "$SB/upstream/B" commit -qm "B: upstream 1"; git -C "$SB/upstream/B" push -q origin main 2>/dev/null
git -C "$SB/upstream" add B >/dev/null; git -C "$SB/upstream" commit -qm "super: bump B"
git -C "$SB/upstream" push -q origin main 2>/dev/null

# local's own work: a commit in A, a superproject commit recording it, an unrelated commit, dirt, untracked
printf 'mine\n' > "$SB/local/A/local.txt"; git -C "$SB/local/A" add -A >/dev/null; git -C "$SB/local/A" commit -qm "A: my commit"
git -C "$SB/local" add A >/dev/null; git -C "$SB/local" commit -qm "seal work"
printf 'local\n' > "$SB/local/local.txt"; git -C "$SB/local" add -A >/dev/null; git -C "$SB/local" commit -qm "local unrelated"
printf 'readme edited\n' >> "$SB/local/README.md"
printf 'A edited\n' >> "$SB/local/A/A.txt"
printf 'u\n' > "$SB/local/untracked-super.txt"; printf 'u\n' > "$SB/local/A/untracked-A.txt"
CAP_SUPER="$(git -C "$SB/local" status --porcelain | sort)"
CAP_A="$(git -C "$SB/local/A" status --porcelain | sort)"
A_HEAD0="$(head_of "$SB/local/A")"; SUPER_HEAD0="$(head_of "$SB/local")"
cp -R "$SB/local" "$SB/local.pristine"

section "Assess: the plan says exactly what will happen"
OUT=$(probe sync_plan "$SB/local")
ok "the plan can run" "$(printf '%s' "$OUT" | jqf can_run)" "True"
ok "  2 of mine on top of 3 incoming" "$(printf '%s' "$OUT" | jqf superproject.ahead):$(printf '%s' "$OUT" | jqf superproject.behind)" "2:3"
ok "  the summary says so in words" "$(printf '%s' "$OUT" | jqf summary)" "Your 2 commits are replayed on top of 3 new commits from origin/main, rebasing the submodules"
ok "  my commit that moves A is named" "$(printf '%s' "$OUT" | jqf superproject.own_commits)" "\"touches\": [\"A\"]"
A_PLAN=$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(json.dumps([s for s in d['submodules'] if s['path']=='A'][0]))")
B_PLAN=$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(json.dumps([s for s in d['submodules'] if s['path']=='B'][0]))")
ok "A will be rebased" "$(printf '%s' "$A_PLAN" | jqf action)" "rebase"
ok "  when the superproject stops on its pointer" "$(printf '%s' "$A_PLAN" | jqf predicted_gitlink_conflict)" "True"
ok "  onto the commit upstream records, not A's remote tip" "$(printf '%s' "$A_PLAN" | jqf rebase_onto)" "$(git -C "$SB/local" rev-parse origin/main:A)"
ok "  its uncommitted edit will be stashed around it" "$(printf '%s' "$A_PLAN" | jqf notes)" "stashed and re-applied"
ok "B is a fast-forward" "$(printf '%s' "$B_PLAN" | jqf action)" "fast-forward"
ok "the orphan gitlink is skipped as unmapped" "$(printf '%s' "$OUT" | jqf skipped)" "\"why\": \"unmapped\""
ok "my uncommitted change will be stashed" "$(printf '%s' "$OUT" | jqf superproject.will_stash):$(printf '%s' "$OUT" | jqf superproject.dirty_count)" "True:1"
ok "the fingerprint pins upstream and every target" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;print(len(json.load(sys.stdin)['fingerprint']))")" "3"
OUT=$(probe sync_plan "$SB/local" "nostash")
ok "without stashing a dirty tree is a named blocker" "$(printf '%s' "$OUT" | jqf blockers)" "Turn on stashing"
ok "  and the plan cannot run" "$(printf '%s' "$OUT" | jqf can_run)" "False"

section "Run: my commits end up on top, in the superproject and in A"
OUT=$(probe sync_run "$SB/local")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and says what it did" "$(printf '%s' "$OUT" | jqf headline)" "Synced: 2 commits of yours on top of 3 new commits from origin/main."
ok "  it mentions the stash" "$(printf '%s' "$OUT" | jqf detail)" "stashed and re-applied"
ok "  the state file is gone" "$([ -e "$SB/local/.git/gitswitch-sync.json" ] && echo present || echo gone)" "gone"
ok "the superproject is 0 behind" "$(git -C "$SB/local" rev-list --left-right --count HEAD...origin/main)" "2	0"
ok "  with my commits on top, in order" "$(subjects "$SB/local" origin/main..HEAD)" "local unrelated|seal work|"
ok "  below upstream's" "$(git -C "$SB/local" log --format=%s -5 | tr '\n' '|')" "local unrelated|seal work|super: bump B|super: upstream work|super: bump A|"
ok "A is on main" "$(on_branch "$SB/local/A")" "main"
ok "  with my commit on top of upstream's two" "$(git -C "$SB/local/A" log --format=%s -3 | tr '\n' '|')" "A: my commit|A: upstream 2|A: upstream 1|"
ok "  and my superproject commit records the rebased A" "$(git -C "$SB/local" rev-parse HEAD:A)" "$(head_of "$SB/local/A")"
ok "B is on main at the recorded commit" "$(on_branch "$SB/local/B"):$(git -C "$SB/local" rev-parse HEAD:B)" "main:$(head_of "$SB/local/B")"
ok "git agrees every submodule matches" "$(git -C "$SB/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"
ok "my uncommitted edits are back (superproject)" "$(git -C "$SB/local" status --porcelain | sort)" "$CAP_SUPER"
ok "  and inside A" "$(git -C "$SB/local/A" status --porcelain | sort)" "$CAP_A"
ok "untracked files untouched" "$([ -e "$SB/local/untracked-super.txt" ] && [ -e "$SB/local/A/untracked-A.txt" ] && echo both || echo lost)" "both"
ok "no stash was left behind" "$(git -C "$SB/local" stash list | wc -l | tr -d ' ')" "0"
ok "a backup branch holds the old superproject tip" "$(git -C "$SB/local" rev-parse "$(git -C "$SB/local" branch --list 'gitswitch-before-sync-*' | tr -d ' *')")" "$SUPER_HEAD0"
ok "  and one in A holds its old tip" "$(git -C "$SB/local/A" rev-parse "$(git -C "$SB/local/A" branch --list 'gitswitch-before-sync-*' | tr -d ' *')")" "$A_HEAD0"
ok "  the result lists them with recovery commands" "$(printf '%s' "$OUT" | jqf sync.backups)" "reset --hard gitswitch-before-sync-"
ok "  every check passed" "$(printf '%s' "$OUT" | jqf sync.verified)" "\"dirty_paths_unchanged\": true"
OUT=$(probe sync_plan "$SB/local")
ok "assessing again finds nothing to do" "$(printf '%s' "$OUT" | jqf nothing_to_do)" "True"
ok "  in words" "$(printf '%s' "$OUT" | jqf summary)" "Nothing to sync"

section "A file conflict pauses the sync; abort puts everything back"
restore
git -C "$SB/local/A" checkout -q -- A.txt     # a clean A, so abort can put it back
printf 'shared line 1\nshared line 2 (mine)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "my shared edit"
SUPER_HEAD1="$(head_of "$SB/local")"; A_HEAD1="$(head_of "$SB/local/A")"
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "the sync pauses" "$(printf '%s' "$OUT" | jqf headline)" "Sync paused: conflicts to resolve."
ok "  naming the file" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "shared.txt"
ok "  in the superproject" "$(printf '%s' "$OUT" | jqf sync.needs_user.where_)" "superproject"
ok "  A was already rebased at its stop" "$(git -C "$SB/local/A" log --format=%s -1)" "A: my commit"
ok "  the status carries the paused sync" "$(probe status "$SB/local" | jqf sync.phase)" "rebasing"
ok "  and a push is refused while it is paused" "$(probe push "$SB/local" "" | jqf refusal.code)" "sync-in-progress"
ok "  so is a plain pull" "$(probe pull "$SB/local" "merge" | jqf refusal.code)" "sync-in-progress"
OUT=$(probe sync_abort "$SB/local")
ok "abort reports itself" "$(printf '%s' "$OUT" | jqf headline)" "Sync aborted."
ok "  the superproject is back at its old tip" "$(head_of "$SB/local")" "$SUPER_HEAD1"
ok "  A was put back too (git's own abort would not have)" "$(head_of "$SB/local/A")" "$A_HEAD1"
ok "  A is still on main" "$(on_branch "$SB/local/A")" "main"
ok "  my uncommitted README edit came back with the autostash" "$(git -C "$SB/local" status --porcelain | grep -c 'README.md' || true)" "1"
ok "  untracked files survived" "$([ -e "$SB/local/untracked-super.txt" ] && echo yes || echo no)" "yes"
ok "  no stash left" "$(git -C "$SB/local" stash list | wc -l | tr -d ' ')" "0"
ok "  the state file is gone" "$([ -e "$SB/local/.git/gitswitch-sync.json" ] && echo present || echo gone)" "gone"
ok "  and nothing is in progress" "$(probe status "$SB/local" | jqf operation)" "None"

section "A file conflict, resolved on the page, then Continue sync"
restore
printf 'shared line 1\nshared line 2 (mine)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "my shared edit"
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "paused on shared.txt" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "shared.txt"
OUT=$(probe sync_continue "$SB/local")
ok "continuing before resolving is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-paths"
printf 'shared line 1\nshared line 2 (both)\nshared line 3\n' > "$SB/local/shared.txt"
OUT=$(probe stage "$SB/local" "shared.txt")
ok "staging the resolution is allowed while paused" "$(printf '%s' "$OUT" | jqf ok)" "True"
OUT=$(probe sync_continue "$SB/local")
ok "continue finishes the sync" "$(printf '%s' "$OUT" | jqf headline)" "Synced: 3 commits of yours on top of 3 new commits"
ok "  my resolved commit is on top" "$(git -C "$SB/local" log --format=%s -1)" "my shared edit"
ok "  A and B are aligned" "$(git -C "$SB/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"
ok "  the README edit is back" "$(git -C "$SB/local" status --porcelain | grep -c 'README.md' || true)" "1"

section "A commit upstream already contains is dropped, and the result says so"
restore
printf 'shared line 1\nshared line 2 (upstream)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "same as upstream"
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and names the dropped commit" "$(printf '%s' "$OUT" | jqf detail)" "dropped because upstream already contained it: same as upstream"
ok "  the other two are on top" "$(subjects "$SB/local" origin/main..HEAD)" "local unrelated|seal work|"

section "Blocked states are named, never guessed"
restore
git -C "$SB/local/A" commit -q --allow-empty -m "A: extra, unrecorded"
OUT=$(probe sync_plan "$SB/local")
ok "A checked out ahead of what my commit records is a blocker" "$(printf '%s' "$OUT" | jqf blockers)" "checked out at"
ok "  running anyway is refused" "$(probe sync_run "$SB/local" | jqf refusal.code)" "sync-blocked"
restore
printf 'more\n' > "$SB/local/A/more.txt"; git -C "$SB/local/A" add -A >/dev/null; git -C "$SB/local/A" commit -qm "A: second"
git -C "$SB/local" add A >/dev/null; git -C "$SB/local" commit -qm "seal again"
OUT=$(probe sync_plan "$SB/local")
ok "two of my commits moving A is a blocker" "$(printf '%s' "$OUT" | jqf blockers)" "2 of your commits move this submodule"
restore
git -C "$SB/local" checkout -q --detach
OUT=$(probe sync_plan "$SB/local")
ok "a detached superproject is refused up front" "$(printf '%s' "$OUT" | jqf refusal.code)" "detached-head"

section "Config that would change the meaning is pinned"
restore
git config --global rebase.updateRefs true
git config --global submodule.recurse true
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "the sync still succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  the backup branch did NOT move with the rebase" "$(git -C "$SB/local" rev-parse "$(git -C "$SB/local" branch --list 'gitswitch-before-sync-*' | tr -d ' *')")" "$SUPER_HEAD0"
ok "  submodules stayed on their branch" "$(on_branch "$SB/local/A"):$(on_branch "$SB/local/B")" "main:main"
git config --global --unset rebase.updateRefs; git config --global --unset submodule.recurse

section "A detached submodule is rebased detached and left where it can be seen"
restore
git -C "$SB/local/A" checkout -q --detach
OUT=$(probe sync_plan "$SB/local")
ok "the plan says A is detached" "$(printf '%s' "$OUT" | jqf submodules)" "it is detached"
OUT=$(probe sync_run "$SB/local")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  A carries my commit on top of upstream's" "$(git -C "$SB/local/A" log --format=%s -2 | tr '\n' '|')" "A: my commit|A: upstream 2|"
ok "  and was re-attached to main, which stood where it was" "$(on_branch "$SB/local/A")" "main"

section "A repository without submodules: the same flow, no submodule rows"
git init -q --bare "$SB/plain.git"; git clone -q "$SB/plain.git" "$SB/plain" 2>/dev/null
cd "$SB/plain"; echo a > a.txt; git add -A >/dev/null; git commit -qm init; git push -q -u origin main 2>/dev/null
git clone -q "$SB/plain.git" "$SB/plain-up" 2>/dev/null; cd "$SB/plain-up"; echo up > up.txt; git add -A >/dev/null; git commit -qm "their commit"; git push -q origin main 2>/dev/null
cd "$SB/plain"; echo me > me.txt; git add -A >/dev/null; git commit -qm "my commit"; echo dirty >> a.txt
OUT=$(probe sync_plan "$SB/plain")
ok "the plan has no submodule rows" "$(printf '%s' "$OUT" | jqf submodules)" "[]"
OUT=$(probe sync_run "$SB/plain")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf headline)" "Synced: 1 commit of yours on top of 1 new commit"
ok "  my commit is on top" "$(git -C "$SB/plain" log --format=%s -2 | tr '\n' '|')" "my commit|their commit|"
ok "  the dirty edit is back" "$(git -C "$SB/plain" status --porcelain)" " M a.txt"

section "A submodule checked out ahead of what my branch records: blocked, then recorded, then synced"
restore
printf 'bmine\n' > "$SB/local/B/bmine.txt"; git -C "$SB/local/B" add -A >/dev/null; git -C "$SB/local/B" commit -qm "B: my commit"
OUT=$(probe sync_plan "$SB/local")
ok "the plan names the unrecorded checkout" "$(printf '%s' "$OUT" | jqf blockers)" "B: it is checked out at"
ok "  and the remedy" "$(printf '%s' "$OUT" | jqf blockers)" "Record submodule pointers"
OUT=$(probe record_pointers "$SB/local")
ok "recording makes one commit" "$(printf '%s' "$OUT" | jqf headline)" "Recorded 1 submodule pointer(s)."
ok "  which records B" "$(git -C "$SB/local" rev-parse HEAD:B)" "$(head_of "$SB/local/B")"
ok "  only B (my dirty README stays uncommitted)" "$(git -C "$SB/local" status --porcelain | grep -c 'README.md' || true)" "1"
OUT=$(probe sync_plan "$SB/local")
ok "now B is rebased like A" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(sorted((x['path'],x['action']) for x in d['submodules']))")" "[('A', 'rebase'), ('B', 'rebase')]"
OUT=$(probe sync_run "$SB/local")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  B has my commit on top of upstream's" "$(git -C "$SB/local/B" log --format=%s -2 | tr '\n' '|')" "B: my commit|B: upstream 1|"
ok "  and the superproject records both rebased submodules" "$(git -C "$SB/local" rev-parse HEAD:A):$(git -C "$SB/local" rev-parse HEAD:B)" "$(head_of "$SB/local/A"):$(head_of "$SB/local/B")"
ok "  my 3 commits on top" "$(subjects "$SB/local" origin/main..HEAD)" "Record submodule pointers: B|local unrelated|seal work|"

section "A submodule ahead of a pointer nobody moves is left alone and named afterwards"
git clone -q --recurse-submodules "$SB/super.git" "$SB/local2" 2>/dev/null
for s in A B; do git -C "$SB/local2/$s" checkout -q main; done
printf 'bmine\n' > "$SB/local2/B/bmine.txt"; git -C "$SB/local2/B" add -A >/dev/null; git -C "$SB/local2/B" commit -qm "B: my commit"
printf 'later\n' > "$SB/upstream/later.txt"; git -C "$SB/upstream" add -A >/dev/null; git -C "$SB/upstream" commit -qm "super: later work"; git -C "$SB/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$SB/local2")
B_PLAN=$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(json.dumps([s for s in d['submodules'] if s['path']=='B'][0]))")
ok "B is ahead" "$(printf '%s' "$B_PLAN" | jqf action)" "ahead"
ok "  with the reason in words" "$(printf '%s' "$B_PLAN" | jqf reason)" "does not record them yet"
OUT=$(probe sync_run "$SB/local2")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf headline)" "Synced: fast-forwarded to 1 new commit from origin/main."
ok "  and names the pointer the branch does not record" "$(printf '%s' "$OUT" | jqf sync.unrecorded_pointers)" "B"
ok "  B kept my commit" "$(git -C "$SB/local2/B" log --format=%s -1)" "B: my commit"
OUT=$(probe record_pointers "$SB/local2")
ok "recording makes one commit" "$(printf '%s' "$OUT" | jqf headline)" "Recorded 1 submodule pointer(s)."
ok "  which now matches B" "$(git -C "$SB/local2" rev-parse HEAD:B)" "$(head_of "$SB/local2/B")"
ok "  a second recording finds nothing" "$(probe record_pointers "$SB/local2" | jqf refusal.code)" "nothing-to-record"

section "Skipped, unreachable and unfetchable submodules"
restore
git -C "$SB/local" submodule deinit -q -f B >/dev/null 2>&1
OUT=$(probe sync_plan "$SB/local")
ok "a mapped but uninitialised submodule is skipped, not touched" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([x['why'] for x in d['skipped'] if x['path']=='B'])")" "['not-initialised']"
ok "  and the sync can still run" "$(printf '%s' "$OUT" | jqf can_run)" "True"
OUT=$(probe sync_run "$SB/local")
ok "  it succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  B's folder is still empty" "$(ls -A "$SB/local/B" | wc -l | tr -d ' ')" "0"
ok "  and the branch records upstream's B pointer" "$(git -C "$SB/local" rev-parse HEAD:B)" "$(git -C "$SB/local" rev-parse origin/main:B)"
restore
git -C "$SB/local/A" remote set-url origin "$SB/does-not-exist.git"
OUT=$(probe sync_plan "$SB/local")
ok "a submodule whose remote cannot be fetched, with a moving pointer, is blocked" "$(printf '%s' "$OUT" | jqf blockers)" "is not available here and fetching failed"
ok "  the plan still assessed B" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([x['action'] for x in d['submodules'] if x['path']=='B'])")" "['fast-forward']"
restore
# Upstream records a commit of A that was never pushed to A's remote.
printf 'secret\n' > "$SB/upstream/A/unpushed.txt"; git -C "$SB/upstream/A" add -A >/dev/null; git -C "$SB/upstream/A" commit -qm "A: never pushed"
git -C "$SB/upstream" add A >/dev/null; git -C "$SB/upstream" commit -qm "super: bump A to an unpushed commit"; git -C "$SB/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$SB/local")
ok "a pointer to a commit A's remote does not have is blocked" "$(printf '%s' "$OUT" | jqf blockers)" "is not reachable from its remote's branches"
ok "  running anyway is refused" "$(probe sync_run "$SB/local" | jqf refusal.code)" "sync-blocked"
git -C "$SB/upstream" reset -q --hard HEAD~1; git -C "$SB/upstream" push -q --force origin main 2>/dev/null   # undo, sandbox-only remote
git -C "$SB/upstream/A" reset -q --hard HEAD~1
restore
ok "the Rebase pull mode still refuses a dirty tree" "$(probe pull "$SB/local" rebase | jqf refusal.code)" "dirty-tree"

section "A conflict inside a submodule pauses there; Continue sync finishes"
restore
# Upstream's A also edits A.txt line 2; my A commit edits the same line.
git -C "$SB/upstream/A" pull -q --rebase origin main 2>/dev/null
printf 'A v1\nline2 (upstream)\nline3\n' > "$SB/upstream/A/A.txt"; git -C "$SB/upstream/A" commit -qam "A: upstream edits A.txt"; git -C "$SB/upstream/A" push -q origin main 2>/dev/null
git -C "$SB/upstream" add A >/dev/null; git -C "$SB/upstream" commit -qm "super: bump A again"; git -C "$SB/upstream" push -q origin main 2>/dev/null
git -C "$SB/local" reset -q --hard HEAD~1            # drop "local unrelated" so one commit of mine moves A
git -C "$SB/local/A" checkout -q -- A.txt
printf 'A v1\nline2 (mine)\nline3\n' > "$SB/local/A/A.txt"; git -C "$SB/local/A" commit -qam "A: my edit of A.txt"
git -C "$SB/local" add A >/dev/null; git -C "$SB/local" commit -q --amend --no-edit
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "the sync pauses inside A" "$(printf '%s' "$OUT" | jqf headline)" "Sync paused inside A."
ok "  saying where to resolve" "$(printf '%s' "$OUT" | jqf sync.needs_user.hint)" "Open it as its own repository"
ok "  A has the conflict" "$(git -C "$SB/local/A" status --porcelain | grep -c '^UU' || true)" "1"
printf 'A v1\nline2 (both)\nline3\n' > "$SB/local/A/A.txt"; git -C "$SB/local/A" add A.txt
OUT=$(probe sync_continue "$SB/local")
ok "continue finishes the sync" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  A has my edits on top of upstream's" "$(git -C "$SB/local/A" log --format=%s -3 | tr '\n' '|')" "A: my edit of A.txt|A: my commit|A: upstream edits A.txt|"
ok "  and the superproject records the rebased A" "$(git -C "$SB/local" rev-parse HEAD:A)" "$(head_of "$SB/local/A")"
ok "  everything aligned" "$(git -C "$SB/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"

verify_result
