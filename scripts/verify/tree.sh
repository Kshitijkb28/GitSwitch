#!/bin/bash
# Tree suite: branches, undo, reset, detach, revert, cherry-pick, conflict
# sides, discard-all and commit lookup — against real git, in throwaway repos
# with an isolated HOME. Local bare repos stand in for remotes; nothing here
# ever reaches the network. Every JSON assertion is paired with a real
# `git -C` check of the same fact.
set -u
source "$(dirname "$0")/lib.sh"
verify_init tree "Tree Tester" "tree@example.test"
SB="$(realdir "$SB")"
W="$SB/work"

subj() { git -C "$1" log -1 --format=%s; }
head_of() { git -C "$1" rev-parse HEAD; }
on_branch() { git -C "$1" symbolic-ref -q --short HEAD 2>/dev/null || echo DETACHED; }
# 'clean' rather than nothing: the ok helper cannot match an empty expectation.
porcelain() { local p; p="$(git -C "$1" status --porcelain --ignore-submodules=none | sort | tr '\n' '|')"; echo "${p:-clean}"; }
# The app's own recovery string, run where it says.
run_recovery() { (cd "$W" && sh -c "$1" >/dev/null 2>&1); echo "rc=$?"; }
# Length of a JSON list at a dotted key.
jlen() { "$PY" -c "
import sys,json
d=json.load(sys.stdin)
for k in '$1'.split('.'):
    d=d.get(k) if isinstance(d,dict) else None
print(len(d) if isinstance(d,list) else 'None')"; }
# The status entry for one path.
entry() { "$PY" -c "
import sys,json
d=json.load(sys.stdin)
m=[e for e in d['entries'] if e['path']=='$1']
print(json.dumps(m[0].get('$2')) if m else 'MISSING')"; }

# --- fixture ------------------------------------------------------------------
for r in remote sub uninit orphan; do git init -q --bare "$SB/$r.git"; done
for r in sub uninit orphan; do
  git clone -q "$SB/$r.git" "$SB/seed/$r" 2>/dev/null
  printf '%s v1\n' "$r" > "$SB/seed/$r/file.txt"
  git -C "$SB/seed/$r" add -A >/dev/null; git -C "$SB/seed/$r" commit -qm "$r initial"; git -C "$SB/seed/$r" push -q origin main 2>/dev/null
done
git clone -q "$SB/remote.git" "$W" 2>/dev/null
printf 'line1\n' > "$W/a.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "first"
printf 'b1\n' > "$W/b.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "second"
printf '.env\n' > "$W/.gitignore"; printf 'SECRET=1\n' > "$W/.env"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "third"
git -C "$W" push -q -u origin main 2>/dev/null
ROOT_SHA="$(git -C "$W" rev-parse HEAD~2)"
git -C "$W" submodule add -q "$SB/sub.git" vendor/sub 2>/dev/null
git -C "$W" submodule add -q "$SB/orphan.git" vendor/orphan 2>/dev/null
git -C "$W" config -f .gitmodules --remove-section submodule.vendor/orphan
git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "add submodules"; git -C "$W" push -q origin main 2>/dev/null
# A second clone adds a mapped submodule; work pulls the gitlink but never initialises it.
git clone -q "$SB/remote.git" "$SB/other" 2>/dev/null
git -C "$SB/other" submodule add -q "$SB/uninit.git" vendor/uninit 2>/dev/null
git -C "$SB/other" add -A >/dev/null; git -C "$SB/other" commit -qm "add uninit submodule"; git -C "$SB/other" push -q origin main 2>/dev/null
git -C "$W" pull -q --ff-only 2>/dev/null
PUSHED_TIP="$(head_of "$W")"

section "origin/HEAD is a pointer, not a branch"
git -C "$W" remote set-head origin main
ok "the fixture has origin/HEAD, as every real clone does" "$(git -C "$W" symbolic-ref refs/remotes/origin/HEAD)" "refs/remotes/origin/main"
ok "  whose short name is just 'origin' (the trap)" "$(git -C "$W" for-each-ref --format='%(refname:short)' refs/remotes/origin/HEAD)" "origin"
OUT=$(probe branches "$W")
ok "the branch list has no 'origin' row for it" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([b["name"] for b in json.load(sys.stdin) if b["is_remote"]])')" "['origin/main']"
OUT=$(probe merge_info "$W" "main")
ok "  nor does the merged-into list" "$(printf '%s' "$OUT" | jqf merged_into)" "[\"origin/main\"]"

section "Submodule statuses: the populated gitlinks, nothing invented"
OUT=$(probe sub_statuses "$W")
ok "exactly the populated gitlinks are listed" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(sorted(x["path"] for x in json.load(sys.stdin)))')" "['vendor/orphan', 'vendor/sub']"
ok "  the mapped-but-uninitialised one is absent" "$(printf '%s' "$OUT" | grep -c 'vendor/uninit' || true)" "0"
ok "  git agrees its folder is empty" "$(ls -A "$W/vendor/uninit" | wc -l | tr -d ' ')" "0"
ok "  the unmapped one is flagged" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([x["listed"] for x in json.load(sys.stdin) if x["path"]=="vendor/orphan"])')" "[False]"
ok "  vendor/sub is on main" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([x["status"]["branch"] for x in json.load(sys.stdin) if x["path"]=="vendor/sub"])')" "['main']"
printf 'edit\n' >> "$W/vendor/sub/file.txt"
OUT=$(probe sub_statuses "$W")
ok "a dirty file inside counts" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([x["status"]["unstaged_count"] for x in json.load(sys.stdin) if x["path"]=="vendor/sub"])')" "[1]"
ok "  git sees it too" "$(git -C "$W/vendor/sub" status --porcelain)" " M file.txt"
OUT=$(probe stage "$W/vendor/sub" "file.txt")
ok "staging inside the submodule works" "$(printf '%s' "$OUT" | jqf headline)" "Staged."
OUT=$(probe commit "$W/vendor/sub" "inside sub")
ok "  and so does committing there" "$(printf '%s' "$OUT" | jqf headline)" "Committed"
ok "  git has the commit" "$(subj "$W/vendor/sub")" "inside sub"
OUT=$(probe status "$W")
ok "the superproject's gitlink entry reports the moved pointer" "$(printf '%s' "$OUT" | entry vendor/sub sub_commit_changed)" "true"
ok "  git shows the gitlink modified" "$(git -C "$W" status --porcelain -- vendor/sub)" " M vendor/sub"
git -C "$W" add vendor/sub >/dev/null; git -C "$W" commit -qm "bump sub"
BEFORE_BUMP="$(git -C "$W" rev-parse HEAD~1)"

section "Branches: create, switch, delete, rename"
OUT=$(probe tree_branch_create "$W" "feature,,switch")
ok "create+switch lands on the new branch" "$(printf '%s' "$OUT" | jqf headline)" "Created and switched to feature."
ok "  git is on it" "$(on_branch "$W")" "feature"
ok "  the outcome names the operation" "$(printf '%s' "$OUT" | jqf tree.op)" "create-branch"
ok "  and how to undo it" "$(printf '%s' "$OUT" | jqf tree.recovery)" "git switch main && git branch -D feature"
printf 'feature edit\n' >> "$W/a.txt"; git -C "$W" commit -qam "feature edit"
printf 'dirt\n' >> "$W/b.txt"
OUT=$(probe tree_branch_switch "$W" "main")
ok "switching with a dirty file the branches agree on is allowed" "$(printf '%s' "$OUT" | jqf headline)" "Switched to main."
ok "  git is on main" "$(on_branch "$W")" "main"
ok "  the dirt came along" "$(git -C "$W" status --porcelain -- b.txt)" " M b.txt"
ok "  the way back is named" "$(printf '%s' "$OUT" | jqf tree.recovery)" "git switch feature"
printf 'collides\n' >> "$W/a.txt"
OUT=$(probe tree_branch_switch "$W" "feature")
ok "a dirty file the switch would overwrite is refused by git and explained" "$(printf '%s' "$OUT" | jqf advice.action)" "stash-first"
ok "  naming the file" "$(printf '%s' "$OUT" | jqf tree.blocking_files)" "a.txt"
ok "  still on main" "$(on_branch "$W")" "main"
git -C "$W" checkout -q -- a.txt b.txt
OUT=$(probe tree_branch_switch "$W" "-x")
ok "a name starting with '-' is refused before git sees it" "$(printf '%s' "$OUT" | jqf refusal.code)" "invalid-name"
OUT=$(probe tree_branch_switch "$W" "nope")
ok "a branch that doesn't exist is explained" "$(printf '%s' "$OUT" | jqf advice.headline)" "That branch doesn't exist here."
OUT=$(probe tree_branch_delete "$W" "main")
ok "deleting the current branch is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "current-branch"
OUT=$(probe tree_branch_delete "$W" "feature")
ok "deleting a branch with its own commit is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-branch"
ok "  with the count" "$(printf '%s' "$OUT" | jqf refusal.message)" "1 commit"
ok "  and the branch still exists" "$(git -C "$W" branch --list feature | tr -d ' ')" "feature"
FEATURE_TIP="$(git -C "$W" rev-parse feature)"
# Merged into develop but not into main: git's own -d refuses, the app's gate
# (no commit that nothing else holds) does not — and must not run -d.
git -C "$W" branch develop feature
ok "git's own -d still refuses it (merged into develop, not into main)" "$(git -C "$W" branch -d feature 2>&1; echo "rc=$?")" "not fully merged"
OUT=$(probe tree_branch_delete "$W" "feature")
ok "the app deletes it without force: another branch holds every commit" "$(printf '%s' "$OUT" | jqf headline)" "Deleted feature."
ok "  git no longer lists it" "$(git -C "$W" branch --list feature | wc -l | tr -d ' ')" "0"
ok "  develop still holds the commit" "$(git -C "$W" rev-parse develop)" "$FEATURE_TIP"
ok "  running the recovery brings it back" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.recovery)")" "rc=0"
git -C "$W" branch -q -D develop
OUT=$(probe tree_branch_delete "$W" "feature")
ok "with develop gone the commit is unique again, so it is refused again" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-branch"
OUT=$(probe tree_branch_delete "$W" "feature,force")
ok "confirming deletes it" "$(printf '%s' "$OUT" | jqf headline)" "Deleted feature."
ok "  the recovery recreates it" "$(printf '%s' "$OUT" | jqf tree.recovery)" "git branch feature $FEATURE_TIP"
ok "  git no longer lists it" "$(git -C "$W" branch --list feature | wc -l | tr -d ' ')" "0"
ok "  running the recovery brings it back" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.recovery)")" "rc=0"
ok "  at the same commit" "$(git -C "$W" rev-parse feature)" "$FEATURE_TIP"
OUT=$(probe tree_branch_rename "$W" "feature,feat2")
ok "rename reports itself" "$(printf '%s' "$OUT" | jqf headline)" "Renamed feature to feat2."
ok "  git lists the new name only" "$(git -C "$W" branch --list 'feat*' | tr -d ' \n')" "feat2"
OUT=$(probe tree_branch_rename "$W" "ghost,x")
ok "renaming a missing branch is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-such-branch"
OUT=$(probe tree_branch_create "$W" "feat2")
ok "creating a name that exists is explained" "$(printf '%s' "$OUT" | jqf advice.headline)" "already exists"
OUT=$(probe tree_branch_create "$W" "nowhere,zzzz")
ok "an unknown start point is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "not-a-commit"
ok "  naming what was typed" "$(printf '%s' "$OUT" | jqf refusal.message)" "\`zzzz\` isn't a commit"

section "Detached HEAD: commits nobody holds are never left behind"
git -C "$W" switch -q --detach; git -C "$W" commit -q --allow-empty -m "loose commit"
OUT=$(probe tree_branch_switch "$W" "main")
ok "switching away from a detached commit is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "detached-commits"
ok "  with the count" "$(printf '%s' "$OUT" | jqf refusal.message)" "1 commit(s)"
ok "  HEAD is still detached" "$(on_branch "$W")" "DETACHED"
OUT=$(probe tree_branch_create "$W" "keeper,,switch")
ok "starting a branch here is the way out" "$(printf '%s' "$OUT" | jqf headline)" "Created and switched to keeper."
ok "  git is on it, at the loose commit" "$(on_branch "$W"):$(subj "$W")" "keeper:loose commit"
probe tree_branch_switch "$W" "main" >/dev/null
ok "  and switching to main now works" "$(on_branch "$W")" "main"

section "A switch that leaves a submodule pointing elsewhere says so"
git -C "$W" branch oldsub "$BEFORE_BUMP"
OUT=$(probe tree_branch_switch "$W" "oldsub")
ok "the switch succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and names the mismatched submodule" "$(printf '%s' "$OUT" | jqf tree.submodule_mismatch)" "vendor/sub"
ok "  in words" "$(printf '%s' "$OUT" | jqf detail)" "Align submodules checks out the commits it records"
ok "  git shows the gitlink modified" "$(git -C "$W" status --porcelain -- vendor/sub)" " M vendor/sub"
OUT=$(probe tree_branch_switch "$W" "main")
ok "back on main nothing mismatches" "$(printf '%s' "$OUT" | jqf tree.submodule_mismatch)" "[]"

section "Undo the last commit"
OUT=$(probe tree_undo_commit "$W")
ok "undo reports itself" "$(printf '%s' "$OUT" | jqf headline)" "Undid the last commit."
ok "  the commit's change is staged again" "$(printf '%s' "$OUT" | jqf staged)" "1"
ok "  git's HEAD is the previous commit" "$(subj "$W")" "add uninit submodule"
ok "  and the gitlink change is in the index" "$(git -C "$W" diff --cached --name-only)" "vendor/sub"
ok "  the redo command is named" "$(printf '%s' "$OUT" | jqf tree.recovery)" "reset --soft"
ok "  running it redoes the commit" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.recovery)")" "rc=0"
ok "  git is back on it" "$(subj "$W")" "bump sub"
git -C "$W" push -q origin main 2>/dev/null
OUT=$(probe tree_undo_commit "$W")
ok "a pushed commit cannot be undone" "$(printf '%s' "$OUT" | jqf refusal.code)" "already-pushed"
ok "  it points at Revert" "$(printf '%s' "$OUT" | jqf refusal.message)" "Revert it instead"
git init -q "$SB/root"; printf 'r\n' > "$SB/root/r.txt"; git -C "$SB/root" add -A >/dev/null; git -C "$SB/root" commit -qm "only commit"
OUT=$(probe tree_undo_commit "$SB/root")
ok "the first commit cannot be undone" "$(printf '%s' "$OUT" | jqf refusal.code)" "root-commit"
git -C "$W" switch -q -c side; printf 'side\n' >> "$W/a.txt"; git -C "$W" commit -qam "side work"
git -C "$W" switch -q main; printf 'm\n' > "$W/m.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "main work"
git -C "$W" merge -q --no-ff side -m "merge side"
MERGE_SHA="$(head_of "$W")"; FIRST_PARENT="$(git -C "$W" rev-parse HEAD^1)"
OUT=$(probe tree_undo_commit "$W")
ok "a merge commit cannot be undone" "$(printf '%s' "$OUT" | jqf refusal.code)" "merge-commit"
ok "  it names the first parent to reset to" "$(printf '%s' "$OUT" | jqf refusal.message)" "$(git -C "$W" rev-parse --short "$FIRST_PARENT")"
ok "  git confirms it has two parents" "$(git -C "$W" rev-list --parents -n1 HEAD | wc -w | tr -d ' ')" "3"

section "Reset: soft, mixed, hard — always with a backup"
OUT=$(probe tree_reset "$W" "HEAD~1,soft")
ok "a soft reset keeps the changes staged" "$(printf '%s' "$OUT" | jqf staged)" "1"
ok "  git agrees" "$(git -C "$W" diff --cached --name-only)" "a.txt"
ok "  HEAD moved to the first parent" "$(head_of "$W")" "$FIRST_PARENT"
BK="$(printf '%s' "$OUT" | jqf tree.backup.branch)"
ok "  a backup branch was made" "$BK" "gitswitch-before-reset-"
ok "  pointing at the old HEAD" "$(git -C "$W" rev-parse "$BK")" "$MERGE_SHA"
ok "  the dropped commit is listed" "$(printf '%s' "$OUT" | jqf tree.dropped)" "merge side"
ok "  the recovery uses the same mode" "$(printf '%s' "$OUT" | jqf tree.recovery)" "reset --soft $BK"
ok "  running it restores the merge" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.recovery)")" "rc=0"
ok "  git is back at the merge, clean" "$(head_of "$W"):$(porcelain "$W")" "$MERGE_SHA:clean"
git -C "$W" branch -q -D "$BK"
OUT=$(probe tree_reset "$W" "HEAD~1,mixed")
ok "a mixed reset leaves the changes unstaged" "$(printf '%s' "$OUT" | jqf staged):$(printf '%s' "$OUT" | jqf unstaged)" "0:1"
ok "  git agrees" "$(git -C "$W" status --porcelain -- a.txt)" " M a.txt"
BK="$(printf '%s' "$OUT" | jqf tree.backup.branch)"
ok "  running the recovery restores the merge" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.recovery)")" "rc=0"
ok "  git is back at the merge, clean" "$(head_of "$W"):$(porcelain "$W")" "$MERGE_SHA:clean"
git -C "$W" branch -q -D "$BK"
printf 'dirt\n' >> "$W/b.txt"
OUT=$(probe tree_reset "$W" "HEAD~1,hard")
ok "a hard reset without a stash on a dirty tree is the user's explicit choice: it runs" "$(printf '%s' "$OUT" | jqf headline)" "Reset to $(git -C "$W" rev-parse --short "$FIRST_PARENT") (hard)."
ok "  the dirt is gone" "$(porcelain "$W")" "clean"
ok "  HEAD moved" "$(head_of "$W")" "$FIRST_PARENT"
ok "  no stash was made" "$(printf '%s' "$OUT" | jqf tree.stash):$(git -C "$W" stash list | wc -l | tr -d ' ')" "None:0"
BK="$(printf '%s' "$OUT" | jqf tree.backup.branch)"
ok "  the backup holds the old HEAD" "$(git -C "$W" rev-parse "$BK")" "$MERGE_SHA"
ok "  running the recovery restores the merge" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.recovery)")" "rc=0"
ok "  git is back at the merge, clean" "$(head_of "$W"):$(porcelain "$W")" "$MERGE_SHA:clean"
git -C "$W" branch -q -D "$BK"
printf 'dirt\n' >> "$W/b.txt"; printf 'new\n' > "$W/untracked.txt"
OUT=$(probe tree_reset "$W" "HEAD~1,hard,stash")
ok "with 'stash first' the dirt is kept" "$(printf '%s' "$OUT" | jqf headline)" "Reset to $(git -C "$W" rev-parse --short "$FIRST_PARENT") (hard)."
ok "  in stash@{0}" "$(git -C "$W" stash list --format=%gs -1)" "gitswitch before reset"
ok "  the result names the stash" "$(printf '%s' "$OUT" | jqf tree.stash.ref)" "stash@{0}"
ok "  the untracked file stays on disk: no reset touches it" "$(porcelain "$W")" "?? untracked.txt|"
ok "  and it is not in the stash (two parents, no untracked tree)" "$(git -C "$W" rev-list --parents -n1 'stash@{0}' | wc -w | tr -d ' ')" "3"
BK="$(printf '%s' "$OUT" | jqf tree.backup.branch)"
ok "  the backup holds the old HEAD" "$(git -C "$W" rev-parse "$BK")" "$MERGE_SHA"
ok "  and its recovery is a hard reset to it" "$(printf '%s' "$OUT" | jqf tree.backup.recovery)" "reset --hard $BK"
ok "  running it restores the merge" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.backup.recovery)")" "rc=0"
git -C "$W" stash pop -q
ok "  and the stash pops the dirt back" "$(git -C "$W" status --porcelain -- b.txt)" " M b.txt"
git -C "$W" checkout -q -- b.txt; rm -f "$W/untracked.txt"; git -C "$W" branch -q -D "$BK"
OUT=$(probe tree_reset "$W" "origin/main~1,hard")
ok "resetting below a pushed commit is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "would-drop-pushed"
ok "  naming the upstream" "$(printf '%s' "$OUT" | jqf refusal.message)" "origin/main"
ok "  HEAD did not move" "$(head_of "$W")" "$MERGE_SHA"
OUT=$(probe tree_resolve "$W" "$(git -C "$W" rev-parse origin/main~1)")
ok "looking that commit up says what a reset would drop" "$(printf '%s' "$OUT" | jqf dropped_if_reset)" "4"
ok "  including pushed commits" "$(printf '%s' "$OUT" | jqf would_drop_pushed)" "True"
ok "  it is on the remote and behind HEAD" "$(printf '%s' "$OUT" | jqf on_remote):$(printf '%s' "$OUT" | jqf contained_in_head)" "True:True"
OUT=$(probe tree_resolve "$W" "HEAD")
ok "HEAD itself is a merge with two parents" "$(printf '%s' "$OUT" | jqf is_merge):$(printf '%s' "$OUT" | jlen parents):$(printf '%s' "$OUT" | jqf is_head)" "True:2:True"
ok "  and drops nothing" "$(printf '%s' "$OUT" | jqf dropped_if_reset)" "0"
ok "unknown text resolves to nothing" "$(probe tree_resolve "$W" "zzzz")" "null"
ok "  text starting with '-' is an error, not a lookup" "$(probe tree_resolve "$W" "-x")" "error"
OUT=$(probe tree_reset "$W" "@{u},hard")
ok "reset to the upstream succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  HEAD is the upstream tip" "$(head_of "$W")" "$(git -C "$W" rev-parse origin/main)"
ok "  three commits left the branch" "$(printf '%s' "$OUT" | jqf tree.dropped_total)" "3"
BK="$(printf '%s' "$OUT" | jqf tree.backup.branch)"
ok "  the backup still holds the merge" "$(git -C "$W" rev-parse "$BK")" "$MERGE_SHA"
OUT=$(probe tree_reset "$W" "HEAD,soft")
ok "a reset to HEAD is nothing to do" "$(printf '%s' "$OUT" | jqf refusal.code)" "nothing-to-do"

section "A hard reset never overwrites an untracked file without saying so"
# A commit adds gen/out.txt; step back below it and recreate the file by hand,
# exactly what a build tool does. A forward `git reset --hard` would replace
# it without a word: git protects only the files it tracks.
mkdir -p "$W/gen"; printf 'committed\n' > "$W/gen/out.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "adds gen/out.txt"
FWD="$(head_of "$W")"
git -C "$W" reset -q --hard HEAD~1
mkdir -p "$W/gen"; printf 'mine, unsaved\n' > "$W/gen/out.txt"
OUT=$(probe tree_reset "$W" "$FWD,hard")
ok "the reset is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "untracked-in-the-way"
ok "  naming the file" "$(printf '%s' "$OUT" | jqf refusal.message)" "gen/out.txt"
ok "  the file is untouched" "$(cat "$W/gen/out.txt")" "mine, unsaved"
ok "  and HEAD did not move" "$(head_of "$W")" "$(git -C "$W" rev-parse "$FWD~1")"
git -C "$W" add gen/out.txt
OUT=$(probe tree_reset "$W" "$FWD,hard")
ok "a STAGED file at that path is the person's stated choice: the reset runs" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and the committed content is in place" "$(cat "$W/gen/out.txt")" "committed"
git -C "$W" reset -q --hard HEAD~1
mkdir -p "$W/gen"; printf 'gen/\n' >> "$W/.gitignore"; printf 'ignored, unsaved\n' > "$W/gen/out.txt"
OUT=$(probe tree_reset "$W" "$FWD,hard")
ok "an ignored file in the way is just as lost, so it is refused too" "$(printf '%s' "$OUT" | jqf refusal.code)" "untracked-in-the-way"
ok "  and kept" "$(cat "$W/gen/out.txt")" "ignored, unsaved"
git -C "$W" checkout -q -- .gitignore; rm -rf "$W/gen"
OUT=$(probe tree_reset "$W" "$FWD,hard")
ok "with nothing in the way the same reset runs" "$(printf '%s' "$OUT" | jqf ok)" "True"
git -C "$W" reset -q --hard HEAD~1

section "Pushed means on any remote branch — for reset and undo alike"
printf 'elsewhere\n' > "$W/else.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "pushed elsewhere"
git -C "$W" push -q origin main:refs/heads/backup 2>/dev/null
ok "the fixture: HEAD is on origin/backup, not on the upstream" "$(git -C "$W" branch -r --contains HEAD | tr -d ' ' | tr '\n' '|'):$(git -C "$W" merge-base --is-ancestor HEAD origin/main; echo "rc=$?")" "origin/backup|:rc=1"
OUT=$(probe tree_reset "$W" "HEAD~1,hard")
ok "a reset that would drop it is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "would-drop-pushed"
ok "  naming the branch that holds it, not the upstream" "$(printf '%s' "$OUT" | jqf refusal.message)" "\`origin/backup\` would be dropped"
ok "  HEAD did not move" "$(subj "$W")" "pushed elsewhere"
OUT=$(probe tree_resolve "$W" "HEAD~1")
ok "the lookup gives the same answer" "$(printf '%s' "$OUT" | jqf would_drop_pushed)" "True"
OUT=$(probe tree_undo_commit "$W")
ok "and so does undo" "$(printf '%s' "$OUT" | jqf refusal.code)" "already-pushed"
git -C "$W" push -q origin --delete backup 2>/dev/null; git -C "$W" fetch -q --prune origin 2>/dev/null
ok "once no remote branch holds it" "$(git -C "$W" branch -r --contains HEAD | wc -l | tr -d ' ')" "0"
OUT=$(probe tree_resolve "$W" "HEAD~1")
ok "  the lookup says a reset drops nothing pushed" "$(printf '%s' "$OUT" | jqf would_drop_pushed)" "False"
OUT=$(probe tree_reset "$W" "HEAD~1,hard")
ok "  and the reset runs" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  back at the upstream tip" "$(head_of "$W")" "$(git -C "$W" rev-parse origin/main)"
git -C "$W" branch -q -D "$(printf '%s' "$OUT" | jqf tree.backup.branch)"

section "Detach, then start a branch there"
TARGET="$(git -C "$W" rev-parse HEAD~1)"
OUT=$(probe tree_detach "$W" "$TARGET")
ok "detach reports the commit" "$(printf '%s' "$OUT" | jqf headline)" "Looking at $(git -C "$W" rev-parse --short "$TARGET")"
ok "  the status is detached" "$(printf '%s' "$OUT" | jqf detached)" "True"
ok "  git agrees" "$(on_branch "$W"):$(head_of "$W")" "DETACHED:$TARGET"
ok "  the way back is named" "$(printf '%s' "$OUT" | jqf tree.recovery)" "git switch main"
OUT=$(probe tree_branch_create "$W" "here,$TARGET,switch")
ok "a branch started at that commit lands on it" "$(on_branch "$W"):$(head_of "$W")" "here:$TARGET"
ok "  the result says so" "$(printf '%s' "$OUT" | jqf headline)" "Created and switched to here."
git -C "$W" switch -q main; git -C "$W" branch -q -D here
OUT=$(probe tree_detach "$W" "zzzz")
ok "an unknown target is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "not-a-commit"

section "Revert"
printf 'plain\n' >> "$W/b.txt"; git -C "$W" commit -qam "plain edit"
PLAIN_SHA="$(head_of "$W")"
OUT=$(probe tree_revert "$W" "HEAD")
ok "a plain revert makes a new commit" "$(printf '%s' "$OUT" | jqf headline)" "Reverted $(git -C "$W" rev-parse --short "$PLAIN_SHA")."
ok "  git has it" "$(subj "$W")" "Revert \"plain edit\""
ok "  the file is back" "$(cat "$W/b.txt")" "b1"
ok "  the recovery drops the revert but keeps its changes staged" "$(printf '%s' "$OUT" | jqf tree.recovery)" "git reset --soft $PLAIN_SHA"
git -C "$W" switch -q -c rv; printf 'r\n' > "$W/r.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "rv work"
git -C "$W" switch -q main; printf 'm2\n' > "$W/m2.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "main again"
git -C "$W" merge -q --no-ff rv -m "merge rv"
RV_MERGE="$(head_of "$W")"
OUT=$(probe tree_revert "$W" "HEAD")
ok "reverting a merge needs a mainline" "$(printf '%s' "$OUT" | jqf refusal.code)" "merge-commit-needs-mainline"
ok "  both parents are offered" "$(printf '%s' "$OUT" | jlen tree.parents)" "2"
ok "  in words" "$(printf '%s' "$OUT" | jqf refusal.message)" "1 keeps \`$(git -C "$W" rev-parse --short HEAD^1) main again\`"
ok "  nothing was committed" "$(head_of "$W")" "$RV_MERGE"
OUT=$(probe tree_revert "$W" "HEAD,1")
ok "with mainline 1 it succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  git has the revert" "$(subj "$W")" "Revert \"merge rv\""
ok "  and the merged-in file is gone" "$([ -e "$W/r.txt" ] && echo present || echo gone)" "gone"
printf 'one\n' > "$W/x.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "x one"
printf 'two\n' > "$W/x.txt"; git -C "$W" commit -qam "x two"
OUT=$(probe tree_revert "$W" "HEAD~1")
ok "a revert that conflicts stops with the revert in progress" "$(printf '%s' "$OUT" | jqf operation.kind)" "revert"
ok "  and says so" "$(printf '%s' "$OUT" | jqf advice.action)" "resolve-conflicts"
ok "  naming the file" "$(printf '%s' "$OUT" | jqf tree.conflicts)" "x.txt"
ok "  git shows the conflict" "$(git -C "$W" status --porcelain -- x.txt | cut -c1)" "U"
OUT=$(probe abort "$W")
ok "abort puts it back" "$(printf '%s' "$OUT" | jqf headline)" "Aborted the revert."
ok "  git is clean with nothing in progress" "$(porcelain "$W"):$([ -e "$W/.git/REVERT_HEAD" ] && echo present || echo none)" "clean:none"
printf 's\n' >> "$W/b.txt"; git -C "$W" add b.txt
OUT=$(probe tree_revert "$W" "HEAD")
ok "a staged change blocks a revert" "$(printf '%s' "$OUT" | jqf refusal.code)" "staged-changes"
git -C "$W" reset -q --hard

section "Cherry-pick"
git -C "$SB/other" switch -q -c pickme; printf 'p\n' > "$SB/other/p.txt"; git -C "$SB/other" add -A >/dev/null; git -C "$SB/other" commit -qm "picked change"; git -C "$SB/other" push -q origin pickme 2>/dev/null
git -C "$SB/other" switch -q -c pickme2 main; printf 'p\n' > "$SB/other/p.txt"; git -C "$SB/other" add -A >/dev/null; git -C "$SB/other" commit -qm "same content"; git -C "$SB/other" push -q origin pickme2 2>/dev/null
git -C "$W" fetch -q origin 2>/dev/null
OUT=$(probe tree_cherry_pick "$W" "origin/pickme")
ok "a commit from another branch lands" "$(printf '%s' "$OUT" | jqf headline)" "Picked $(git -C "$W" rev-parse --short origin/pickme)."
ok "  git has it on top" "$(subj "$W"):$(cat "$W/p.txt")" "picked change:p"
ok "  the recovery drops it but keeps its changes staged" "$(printf '%s' "$OUT" | jqf tree.recovery)" "git reset --soft"
OUT=$(probe tree_cherry_pick "$W" "HEAD~1")
ok "an ancestor is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "already-contained"
OUT=$(probe tree_cherry_pick "$W" "origin/pickme2")
ok "a commit whose content is already here is skipped" "$(printf '%s' "$OUT" | jqf headline)" "Nothing to apply"
ok "  nothing is in progress" "$(printf '%s' "$OUT" | jqf operation)" "None"
ok "  git agrees" "$([ -e "$W/.git/CHERRY_PICK_HEAD" ] && echo present || echo none):$(subj "$W")" "none:picked change"
OUT=$(probe tree_cherry_pick "$W" "$MERGE_SHA")
ok "a merge commit is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "merge-commit"

section "Conflict sides: mine and theirs in the user's words"
printf 'base\n' > "$W/c.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "c base"
git -C "$W" switch -q -c cf; printf 'CF\n' > "$W/c.txt"; git -C "$W" commit -qam "c cf"
git -C "$W" switch -q main; printf 'MINE\n' > "$W/c.txt"; git -C "$W" commit -qam "c mine"
C_MINE="$(head_of "$W")"
git -C "$W" merge -q cf >/dev/null 2>&1
ok "the merge conflicts" "$(git -C "$W" status --porcelain -- c.txt)" "UU c.txt"
OUT=$(probe stage "$W" "c.txt")
ok "marking it resolved with the markers still in it is refused by name" "$(printf '%s' "$OUT" | jqf refusal.code)" "conflict-markers"
ok "  naming the line" "$(printf '%s' "$OUT" | jqf refusal.message)" "c.txt still contains conflict markers (line 1)"
ok "  and nothing was staged" "$(git -C "$W" ls-files -u c.txt | wc -l | tr -d ' ')" "3"
OUT=$(probe tree_resolve_side "$W" "mine,c.txt")
ok "Keep mine during a merge keeps my content" "$(cat "$W/c.txt")" "MINE"
ok "  the result says what ran" "$(printf '%s' "$OUT" | jqf tree.mapping_note)" "Keep mine ran \`git checkout --ours\`"
ok "  the file is staged and no longer conflicted" "$(git -C "$W" ls-files -u c.txt | wc -l | tr -d ' '):$(git -C "$W" ls-files -s c.txt | cut -c1-6)" "0:100644"
ok "  no conflicts remain" "$(printf '%s' "$OUT" | jqf conflicted)" "0"
OUT=$(probe tree_resolve_side "$W" "mine,c.txt")
ok "  resolving it again is refused as stale" "$(printf '%s' "$OUT" | jqf refusal.code)" "not-conflicted"
probe abort "$W" >/dev/null
git -C "$W" switch -q -c rb "$C_MINE~1"; printf 'RB\n' > "$W/c.txt"; git -C "$W" commit -qam "c rb"
git -C "$W" rebase -q main >/dev/null 2>&1
ok "the rebase conflicts" "$(git -C "$W" status --porcelain -- c.txt):$([ -d "$W/.git/rebase-merge" ] && echo rebasing || echo idle)" "UU c.txt:rebasing"
ok "  git stores the base as a full id" "$(cat "$W/.git/rebase-merge/onto")" "$C_MINE"
OUT=$(probe status "$W")
ok "  the status names it by its branch instead" "[$(printf '%s' "$OUT" | jqf operation.sides.theirs)]" "[main]"
ok "  in the banner too" "$(printf '%s' "$OUT" | jqf operation.detail)" "rb onto main"
OUT=$(probe tree_resolve_side "$W" "theirs,c.txt")
ok "Take theirs during a rebase takes the upstream's content" "$(cat "$W/c.txt")" "MINE"
ok "  and explains the swap" "$(printf '%s' "$OUT" | jqf tree.mapping_note)" "During a rebase git calls your commit 'theirs', so Take theirs ran \`git checkout --ours\`"
ok "  the file is staged" "$(git -C "$W" diff --name-only --diff-filter=U | wc -l | tr -d ' ')" "0"
probe abort "$W" >/dev/null; git -C "$W" switch -q main
git -C "$W" switch -q -c del; git -C "$W" rm -q c.txt; git -C "$W" commit -qm "delete c"
git -C "$W" switch -q main; printf 'MINE2\n' > "$W/c.txt"; git -C "$W" commit -qam "c mine2"
git -C "$W" merge -q del >/dev/null 2>&1
ok "deleted-by-them conflicts" "$(git -C "$W" status --porcelain -- c.txt)" "UD c.txt"
OUT=$(probe tree_resolve_side "$W" "theirs,c.txt")
ok "Take theirs removes the file" "$(printf '%s' "$OUT" | jqf detail)" "removed because that side deleted it"
ok "  it left the index" "$(git -C "$W" ls-files c.txt | wc -l | tr -d ' ')" "0"
ok "  staged as a deletion" "$(git -C "$W" status --porcelain -- c.txt)" "D  c.txt"
probe abort "$W" >/dev/null
git -C "$W" switch -q -c subA
printf 'A\n' > "$W/vendor/sub/a.txt"; git -C "$W/vendor/sub" add -A >/dev/null; git -C "$W/vendor/sub" commit -qm "sub A"
git -C "$W" add vendor/sub >/dev/null; git -C "$W" commit -qm "record A"
git -C "$W" switch -q main
git -C "$W/vendor/sub" reset -q --hard HEAD~1; printf 'B\n' > "$W/vendor/sub/b.txt"; git -C "$W/vendor/sub" add -A >/dev/null; git -C "$W/vendor/sub" commit -qm "sub B"
git -C "$W" add vendor/sub >/dev/null; git -C "$W" commit -qm "record B"
git -C "$W" merge -q subA >/dev/null 2>&1
ok "a gitlink conflict" "$(git -C "$W" status --porcelain -- vendor/sub)" "UU vendor/sub"
SUB_A="$(git -C "$W" rev-parse subA:vendor/sub)"; SUB_B="$(git -C "$W" rev-parse HEAD:vendor/sub)"
OUT=$(probe status "$W")
ok "  its row names the commit each side would record" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;e=[e for e in json.load(sys.stdin)['entries'] if e['path']=='vendor/sub'][0];print(e.get('pointer_mine'), e.get('pointer_theirs'))")" "$SUB_B $SUB_A"
printf 'x\n' >> "$W/vendor/sub/b.txt"
OUT=$(probe tree_resolve_side "$W" "theirs,vendor/sub")
ok "taking a side over changes inside the submodule is refused" "$(printf '%s' "$OUT" | jqf refusal.code):$(git -C "$W/vendor/sub" rev-parse HEAD)" "submodule-changes:$SUB_B"
git -C "$W/vendor/sub" checkout -q -- b.txt
OUT=$(probe tree_resolve_side "$W" "theirs,vendor/sub")
ok "Take theirs checks the submodule out at the other side's commit and records it" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/vendor/sub" rev-parse HEAD):$(git -C "$W" ls-files -s -- vendor/sub | awk '{print $2"/"$3}')" "True:$SUB_A:$SUB_A/0"
ok "  the conflict is gone" "$(git -C "$W" diff --name-only --diff-filter=U | wc -l | tr -d ' ')" "0"
ok "  the submodule's branch kept its own commit" "$(git -C "$W/vendor/sub" rev-parse main)" "$SUB_B"
ok "  and the result names what it recorded" "$(printf '%s' "$OUT" | jqf detail)" "vendor/sub now records ${SUB_A:0:7}"
OUT=$(probe tree_resolve_side "$W" "mine")
ok "no paths is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-paths"
probe abort "$W" >/dev/null
git -C "$W/vendor/sub" switch -q main
ok "  the merge is aborted" "$(porcelain "$W")" "clean"

section "Discard everything"
printf 'new\n' > "$W/new.txt"; git -C "$W" add new.txt; printf 'mod\n' >> "$W/a.txt"
OUT=$(probe tree_discard_all "$W")
ok "discard restores tracked files" "$(printf '%s' "$OUT" | jqf headline)" "Discarded everything."
ok "  counting them" "$(printf '%s' "$OUT" | jqf tree.restored_tracked)" "2"
ok "  a newly added file survives as untracked" "$(porcelain "$W")" "?? new.txt|"
OUT=$(probe tree_discard_all "$W" "untracked")
ok "with untracked it is deleted" "$([ -e "$W/new.txt" ] && echo present || echo gone)" "gone"
ok "  counted" "$(printf '%s' "$OUT" | jqf tree.deleted_untracked)" "1"
ok "  the tree is clean" "$(porcelain "$W")" "clean"
printf 'inner\n' >> "$W/vendor/sub/file.txt"; printf 'mod\n' >> "$W/a.txt"; printf 'junk\n' > "$W/junk.txt"
OUT=$(probe tree_discard_all "$W" "untracked")
ok "ignored files survive (never clean -x)" "$([ -e "$W/.env" ] && echo kept || echo DELETED)" "kept"
ok "  the submodule's inner change survives" "$(git -C "$W/vendor/sub" status --porcelain)" " M file.txt"
ok "  and is named" "$(printf '%s' "$OUT" | jqf detail)" "Changes inside submodules were not touched: vendor/sub"
ok "  the junk is gone" "$([ -e "$W/junk.txt" ] && echo present || echo gone)" "gone"
git -C "$W/vendor/sub" checkout -q -- file.txt
printf 'mod\n' >> "$W/a.txt"; printf 'keep me\n' > "$W/keep-new.txt"
OUT=$(probe tree_discard_all "$W" "stash")
ok "with a stash first the change is kept" "$(printf '%s' "$OUT" | jqf tree.stash.ref)" "stash@{0}"
ok "  git has it in stash@{0}" "$(git -C "$W" stash show --name-only 'stash@{0}')" "a.txt"
ok "  the new file was not asked to be deleted, so it stays on disk" "$(porcelain "$W")" "?? keep-new.txt|"
ok "  and is not in the stash (two parents, no untracked tree)" "$(git -C "$W" rev-list --parents -n1 'stash@{0}' | wc -w | tr -d ' ')" "3"
ok "  the detail says it was left in place" "$(printf '%s' "$OUT" | jqf detail)" "1 untracked file(s) were left in place"
git -C "$W" stash drop -q
printf 'mod\n' >> "$W/a.txt"
OUT=$(probe tree_discard_all "$W" "untracked,stash")
ok "asked to delete new files, the stash takes them first (a third parent)" "$(git -C "$W" rev-list --parents -n1 'stash@{0}' | wc -w | tr -d ' ')" "4"
ok "  holding the new file" "$(git -C "$W" ls-tree -r --name-only 'stash@{0}^3')" "keep-new.txt"
ok "  which is gone from disk" "$([ -e "$W/keep-new.txt" ] && echo present || echo gone)" "gone"
ok "  and the tree is clean" "$(porcelain "$W")" "clean"
git -C "$W" stash drop -q
OUT=$(probe tree_discard_all "$W")
ok "a clean tree has nothing to discard" "$(printf '%s' "$OUT" | jqf refusal.code)" "nothing-to-discard"
git -C "$W" switch -q -c dz; printf 'DZ\n' > "$W/b.txt"; git -C "$W" commit -qam "b dz"
git -C "$W" switch -q main; printf 'MZ\n' > "$W/b.txt"; git -C "$W" commit -qam "b mz"
git -C "$W" merge -q dz >/dev/null 2>&1
OUT=$(probe tree_discard_all "$W")
ok "during a merge it is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "operation-in-progress"
ok "  pointing at Abort" "$(printf '%s' "$OUT" | jqf refusal.message)" "Abort the merge instead"
git -C "$W" merge --abort

section "History diffs"
OUT=$(probe commit_diff "$W" "$C_MINE,c.txt")
ok "a normal commit shows a hunk" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(" ".join(l["text"] for l in json.load(sys.stdin)["lines"] if l["kind"]=="hunk"))')" "@@"
ok "  git shows the same change" "$(git -C "$W" show --format= "$C_MINE" -- c.txt | grep -c '^+MINE')" "1"
OUT=$(probe commit_diff "$W" "$ROOT_SHA,a.txt")
ok "the root commit diffs against nothing" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(" ".join(l["text"] for l in json.load(sys.stdin)["lines"] if l["kind"]=="add"))')" "+line1"
OUT=$(probe commit_diff "$W" "$RV_MERGE,r.txt")
ok "a merge commit shows its first-parent diff" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(" ".join(l["text"] for l in json.load(sys.stdin)["lines"] if l["kind"]=="add"))')" "+r"
ok "  which git's combined diff would have hidden" "$(git -C "$W" show --format= "$RV_MERGE" -- r.txt | wc -l | tr -d ' ')" "0"
# A rename git detects (mostly-unchanged content, same directory): its numstat
# prints one row in `dir/{old => new}` notation, which is not a path.
mkdir -p "$W/docs"; seq 1 40 > "$W/docs/big.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "big file"
git -C "$W" mv docs/big.txt docs/renamed.txt; printf 'renamed edit\n' >> "$W/docs/renamed.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "rename big"
RENAME_SHA="$(head_of "$W")"
ok "git itself prints the rename as one row in its own notation" "$(git -C "$W" show --numstat --format= "$RENAME_SHA" | cut -f3)" "docs/{big.txt => renamed.txt}"
OUT=$(probe commit_detail "$W" "$RENAME_SHA")
ok "the commit's file list carries real paths instead" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(sorted(f["path"] for f in json.load(sys.stdin)["files"]))')" "['docs/big.txt', 'docs/renamed.txt']"
OUT=$(probe commit_diff "$W" "$RENAME_SHA,docs/renamed.txt")
ok "  so the new path's diff shows the content" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(" ".join(l["text"] for l in json.load(sys.stdin)["lines"] if l["kind"]=="add"))')" "+renamed edit"
OUT=$(probe commit_diff "$W" "$RENAME_SHA,docs/big.txt")
ok "  and the old path's diff shows the deletion" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(" ".join(l["text"] for l in json.load(sys.stdin)["lines"] if l["kind"]=="meta"))')" "deleted file mode"
OUT=$(probe commit_diff "$W" "$RENAME_SHA,docs/{big.txt => renamed.txt}")
ok "  git's notation, sent back as a path, is an empty diff rather than an error" "$(printf '%s' "$OUT" | jqf empty_reason)" "No changes to this file"

section "A renamed file cannot carry its conflict markers into the commit"
M="$SB/markers"; git init -q -b main "$M"
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do printf 'line %s\n' "$i"; done > "$M/r.txt"
git -C "$M" add -A >/dev/null; git -C "$M" commit -qm "r base"
git -C "$M" switch -q -c rf; sed -i.bak 's/^line 10$/line 10 (theirs)/' "$M/r.txt"; rm -f "$M/r.txt.bak"; git -C "$M" commit -qam "r theirs"
git -C "$M" switch -q main; sed -i.bak 's/^line 10$/line 10 (mine)/' "$M/r.txt"; rm -f "$M/r.txt.bak"; git -C "$M" commit -qam "r mine"
git -C "$M" merge -q rf >/dev/null 2>&1
ok "the merge conflicts" "$(git -C "$M" status --porcelain -- r.txt)" "UU r.txt"
# "Resolved" by moving the file away, markers and all.
cp "$M/r.txt" "$M/moved.txt"; git -C "$M" rm -q -f r.txt; git -C "$M" add moved.txt
ok "  git sees a rename, which an edit-only filter would skip" "$(git -C "$M" diff --cached --name-status -M | cut -c1)" "R"
OUT=$(probe continue "$M")
ok "finishing the merge is refused: the markers would be committed" "$(printf '%s' "$OUT" | jqf refusal.code)" "conflict-markers"
ok "  naming the file where they are now, and the line" "$(printf '%s' "$OUT" | jqf refusal.message)" "moved.txt still contains conflict markers (line 10)"
ok "  and the merge is still open" "$([ -e "$M/.git/MERGE_HEAD" ] && echo open || echo closed)" "open"
# A wider marker, as the conflict-marker-size attribute makes git write them.
git -C "$M" merge --abort 2>/dev/null; git -C "$M" reset -q --hard; git -C "$M" clean -fdq
printf 'r.txt conflict-marker-size=10\n' > "$M/.gitattributes"; git -C "$M" add .gitattributes; git -C "$M" commit -qm "wide markers"
git -C "$M" merge -q rf >/dev/null 2>&1
ok "with conflict-marker-size=10 git writes ten" "$(grep -c '^<<<<<<<<<< ' "$M/r.txt")" "1"
OUT=$(probe stage "$M" "r.txt")
ok "  and marking it resolved is still refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "conflict-markers"
ok "  naming the line" "$(printf '%s' "$OUT" | jqf refusal.message)" "r.txt still contains conflict markers (line 10)"
OUT=$(pinvoke changes_conflict_view "{\"repoPath\":\"$M\",\"path\":\"r.txt\"}")
ok "  the conflict view reads the same hunk" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['marker_count'],d['hunks'][0]['mine'],d['hunks'][0]['theirs'])")" "1 ['line 10 (mine)'] ['line 10 (theirs)']"

section "The marker check at Continue does not depend on settings, attributes or the marker's width"
# Each time: a fresh conflict in r.txt, staged from a terminal with its markers, then Continue.
fresh_conflict() { git -C "$M" merge --abort 2>/dev/null; git -C "$M" reset -q --hard; git -C "$M" clean -fdq; git -C "$M" merge -q rf >/dev/null 2>&1; }
staged_then_continue() { git -C "$M" add r.txt; probe continue "$M" | jqf refusal.code; }
fresh_conflict
ok "staged with its markers, Continue is refused" "$(staged_then_continue)" "conflict-markers"
git -C "$M" config color.ui always; fresh_conflict
ok "  also with color.ui=always" "$(staged_then_continue)" "conflict-markers"
git -C "$M" config --unset color.ui; git -C "$M" config color.grep always; fresh_conflict
ok "  and with color.grep=always" "$(staged_then_continue)" "conflict-markers"
git -C "$M" config --unset color.grep
fresh_conflict; git -C "$M" merge --abort
printf 'r.txt -diff conflict-marker-size=10\n' > "$M/.gitattributes"; git -C "$M" commit -qam "no diff for r.txt"
fresh_conflict
ok "  a file an attribute hides from diffs is still read" "$(staged_then_continue)" "conflict-markers"
fresh_conflict; git -C "$M" merge --abort
printf 'r.txt conflict-marker-size=5\n' > "$M/.gitattributes"; git -C "$M" commit -qam "narrow markers"
fresh_conflict
ok "with conflict-marker-size=5 git writes five" "$(grep -c '^<<<<< ' "$M/r.txt")" "1"
ok "  and Continue is refused for them too" "$(staged_then_continue)" "conflict-markers"
fresh_conflict; git -C "$M" merge --abort
git -C "$M" rm -q .gitattributes; git -C "$M" commit -qm "default markers again"
fresh_conflict
# A hand edit that left bare markers, in a file saved with CRLF line endings.
printf 'line 1\r\n<<<<<<<\r\nline 10 (mine)\r\n=======\r\nline 10 (theirs)\r\n>>>>>>>\r\nline 11\r\n' > "$M/r.txt"
OUT=$(git -C "$M" add r.txt 2>/dev/null; probe continue "$M")
ok "bare markers in a CRLF file are refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "conflict-markers"
ok "  naming the line" "$(printf '%s' "$OUT" | jqf refusal.message)" "r.txt still contains conflict markers (line 2)"
ok "  nothing was committed in any of these" "$([ -e "$M/.git/MERGE_HEAD" ] && echo open || echo closed):$(git -C "$M" log --format=%s -1)" "open:default markers again"

section "A path that changed its type is read for markers too"
git -C "$M" merge --abort 2>/dev/null; git -C "$M" reset -q --hard; git -C "$M" clean -fdq
# `link` is a symbolic link in the branch; during the merge it is staged as an ordinary file with markers in it.
LB="$(printf 'r.txt' | git -C "$M" hash-object -w --stdin)"
git -C "$M" update-index --add --cacheinfo "120000,$LB,link"; git -C "$M" commit -qm "a link"; git -C "$M" checkout -q -- link 2>/dev/null
git -C "$M" merge -q rf >/dev/null 2>&1
git -C "$M" checkout -q --ours -- r.txt; git -C "$M" add r.txt
MB="$(printf 'a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> rf\nb\n' | git -C "$M" hash-object -w --stdin)"
git -C "$M" update-index --cacheinfo "100644,$MB,link"
ok "the staged change is a change of type" "$(git -C "$M" diff --cached --raw --no-abbrev -- link | cut -c1-14)" ":120000 100644"
OUT=$(probe continue "$M")
ok "  and Continue is refused for the markers in it" "$(printf '%s' "$OUT" | jqf refusal.code)" "conflict-markers"
ok "  naming the path and the line" "$(printf '%s' "$OUT" | jqf refusal.message)" "link still contains conflict markers (line 2)"
ok "  the merge is still open" "$([ -e "$M/.git/MERGE_HEAD" ] && echo open || echo closed)" "open"

section "Thousands of staged files do not stall Continue"
git -C "$M" merge --abort 2>/dev/null; git -C "$M" reset -q --hard; git -C "$M" clean -fdq
# The other side of a merge adds 4,000 files; one conflict, resolved and staged. The check reads every
# staged file through commands that answer while they are still being told what to read.
git -C "$M" switch -q -c wide rf
"$PY" -c "
import os,sys
d=os.path.join(sys.argv[1],'some','fairly','ordinary','source','dir'); os.makedirs(d)
for i in range(4000):
    with open(os.path.join(d,'file_%05d.txt'%i),'w') as f: f.write('content %d\n'%i)
" "$M"
git -C "$M" add -A >/dev/null; git -C "$M" commit -qm "four thousand files"; git -C "$M" switch -q main
git -C "$M" merge -q wide >/dev/null 2>&1
git -C "$M" checkout -q --ours -- r.txt; git -C "$M" add r.txt
ok "the merge is open with 4,000 files staged" "$([ -e "$M/.git/MERGE_HEAD" ] && echo open || echo closed):$([ "$(git -C "$M" diff --cached --name-only | wc -l)" -ge 4000 ] && echo many || echo few)" "open:many"
( probe continue "$M" > "$SB/wide.out" 2>/dev/null ) & WPID=$!
WAITED=0; while kill -0 "$WPID" 2>/dev/null && [ "$WAITED" -lt 120 ]; do sleep 1; WAITED=$((WAITED+1)); done
if kill -0 "$WPID" 2>/dev/null; then
  # Only the probe's own descendants — gathered before the kill, since a killed parent's children re-parent to pid 1.
  KIDS=""; gather() { local c; for c in $(pgrep -P "$1" 2>/dev/null); do gather "$c"; done; KIDS="$KIDS $1"; }; gather "$WPID"
  kill $KIDS 2>/dev/null
  WIDE="still running after ${WAITED}s"
else
  WIDE="$(jqf ok < "$SB/wide.out"):$(jqf headline < "$SB/wide.out")"
fi
ok "Continue comes back, with the merge finished" "$WIDE" "True:"
ok "  the merge is closed and the files are in it" "$([ -e "$M/.git/MERGE_HEAD" ] && echo open || echo closed):$(git -C "$M" ls-tree -r --name-only HEAD -- some | wc -l | tr -d ' ')" "closed:4000"

section "Continue of a rebase keeps the stopped commit's message, commits it once, and honours the rebase's own options"
# rb_world <dir> <subject of the commit that will conflict>
rb_world() {
  git init -q "$1"; printf 'l1\nl2\nl3\nl4\nl5\nl6\n' > "$1/f.txt"; printf 'n\n' > "$1/notes.txt"; git -C "$1" add -A >/dev/null; git -C "$1" commit -qm base
  git -C "$1" switch -q -c other; printf 'l1\nl2 other\nl3\nl4\nl5\nl6\n' > "$1/f.txt"; git -C "$1" commit -qam "other: l2"
  git -C "$1" switch -q main; printf 'l1\nl2 mine\nl3\nl4\nl5\nl6\n' > "$1/f.txt"; GIT_AUTHOR_DATE="2020-01-01T10:00:00+0000" git -C "$1" commit -qam "$2"
  printf 'l1\nl2 mine\nl3\nl4\nl5\nl6 mine\n' > "$1/f.txt"; GIT_AUTHOR_DATE="2020-01-02T10:00:00+0000" git -C "$1" commit -qam "B more"
}
resolve_rb() { printf 'l1\nl2 both\nl3\nl4\nl5\nl6\n' > "$1/f.txt"; git -C "$1" add f.txt; }
RB="$SB/rb"; rb_world "$RB" "#42 l2 mine"
GIT_EDITOR=true git -C "$RB" -c rebase.backend=merge rebase other >/dev/null 2>&1
resolve_rb "$RB"; printf 'edit\n' >> "$RB/notes.txt"          # an unstaged edit to another tracked file
OUT=$(probe continue "$RB")
ok "an unstaged edit elsewhere: Continue is refused by git, and nothing was committed" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$RB" log --format=%s -1)" "False:other: l2"
git -C "$RB" checkout -q -- notes.txt
OUT=$(probe continue "$RB")
ok "  then Continue finishes: the stopped commit once, its '#' message whole" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$RB" log --format=%s other..HEAD | tr '\n' '|')" "True:B more|#42 l2 mine|"
ok "  and it is not called skipped" "$(printf '%s' "$OUT" | jqf detail | grep -c skipped || true)" "0"
ok "  with the author's date kept" "$(git -C "$RB" log --format=%at -1 HEAD~1)" "1577872800"
RB2="$SB/rb2"; rb_world "$RB2" "#42 l2 mine"
GIT_EDITOR=true git -C "$RB2" -c rebase.backend=merge rebase --signoff other >/dev/null 2>&1
resolve_rb "$RB2"
OUT=$(probe continue "$RB2")
ok "a --signoff rebase: the resolved commit carries the trailer like its siblings" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$RB2" log --format=%B other..HEAD | grep -c '^Signed-off-by: ' || true):$(git -C "$RB2" log --format=%s -1 HEAD~1)" "True:2:#42 l2 mine"
RB3="$SB/rb3"; rb_world "$RB3" "l2 mine"
GIT_EDITOR=true git -C "$RB3" -c rebase.backend=merge rebase --committer-date-is-author-date other >/dev/null 2>&1
resolve_rb "$RB3"
OUT=$(probe continue "$RB3")
ok "a --committer-date-is-author-date rebase keeps its dates: that stop is left to git" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$RB3" log --format='%at=%ct' -1 HEAD~1)" "True:1577872800=1577872800"
# The apply backend (someone's own `git rebase --apply`): a resolution that leaves nothing is still skipped.
RB4="$SB/rb4"; rb_world "$RB4" "l2 mine"
GIT_EDITOR=true git -C "$RB4" rebase --apply other >/dev/null 2>&1
git -C "$RB4" checkout -q --ours -- f.txt 2>/dev/null || git -C "$RB4" show other:f.txt > "$RB4/f.txt"; git -C "$RB4" add f.txt
ok "an apply-backend stop, resolved to what is already there" "$([ -d "$RB4/.git/rebase-apply" ] && echo apply || echo other):$(git -C "$RB4" diff --cached --quiet HEAD && echo empty || echo staged)" "apply:empty"
OUT=$(probe continue "$RB4")
ok "  is skipped by Continue, and said so" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf detail):$(git -C "$RB4" log --format=%s other..HEAD | tr '\n' '|')" "True:The stopped commit had nothing left to apply after the resolution, so it was skipped.:B more|"

section "Going back across a submodule pointer bump: a hard reset offers to align, soft and mixed keep the pointer change"
AL="$SB/align"
git init -q "$AL"; printf 'a\n' > "$AL/a.txt"; git -C "$AL" add -A >/dev/null; git -C "$AL" commit -qm "a"
git -C "$AL" submodule add -q "$SB/sub.git" s 2>/dev/null; git -C "$AL" commit -qm "add s"
S_OLD="$(git -C "$AL" rev-parse HEAD:s)"
printf 'two\n' >> "$AL/s/file.txt"; git -C "$AL/s" commit -qam "s two"; S_NEW="$(git -C "$AL/s" rev-parse HEAD)"
git -C "$AL" add s; git -C "$AL" commit -qm "bump s"; BUMP="$(git -C "$AL" rev-parse HEAD)"
OUT=$(probe tree_reset "$AL" "HEAD~1,mixed")
ok "a mixed reset back over the bump keeps the pointer change, unstaged" "$(printf '%s' "$OUT" | jqf tree.submodule_mismatch):$(git -C "$AL" status --porcelain -- s)" "[]: M s"
ok "  and says so instead of offering to align it away" "$(printf '%s' "$OUT" | jqf detail)" "The submodule pointer change for s is kept with the other changes."
git -C "$AL" reset -q --hard "$BUMP"
OUT=$(probe tree_reset "$AL" "HEAD~1,soft")
ok "a soft reset keeps it staged" "$(printf '%s' "$OUT" | jqf tree.submodule_mismatch):$(git -C "$AL" status --porcelain -- s)" "[]:M  s"
ok "  and says so too" "$(printf '%s' "$OUT" | jqf detail)" "The submodule pointer change for s is kept with the other changes."
git -C "$AL" reset -q --hard "$BUMP"
OUT=$(probe tree_reset "$AL" "HEAD~1,hard")
ok "a hard reset back over the bump names the submodule it left ahead" "$(printf '%s' "$OUT" | jqf tree.submodule_mismatch)" '["s"]'
ok "  and offers to align it" "$(printf '%s' "$OUT" | jqf detail)" "Align submodules checks out the commits it records"
ok "  git agrees s sits elsewhere" "$(git -C "$AL" status --porcelain -- s)" " M s"
printf 'edit\n' >> "$AL/s/file.txt"
OUT=$(probe tree_sub_align "$AL" "s")
ok "aligning a submodule with changes inside is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "submodule-changes"
ok "  and nothing moved" "$(git -C "$AL/s" rev-parse HEAD)" "$S_NEW"
git -C "$AL/s" checkout -q -- file.txt
git -C "$AL/s" switch -q --detach; git -C "$AL/s" commit -q --allow-empty -m "loose in s"; LOOSE="$(git -C "$AL/s" rev-parse HEAD)"
OUT=$(probe tree_sub_align "$AL" "s")
ok "aligning a submodule whose detached HEAD holds a commit no branch has is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "submodule-detached-commits"
ok "  with the count" "$(printf '%s' "$OUT" | jqf refusal.message)" "1 commit(s) at s's checkout"
ok "  and nothing moved" "$(git -C "$AL/s" rev-parse HEAD)" "$LOOSE"
git -C "$AL/s" switch -q main 2>/dev/null
OUT=$(probe tree_sub_align "$AL" "s")
ok "Align checks out the commit the repository records" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$AL/s" rev-parse HEAD)" "True:$S_OLD"
ok "  the repository is clean again" "$(porcelain "$AL")" "clean"
ok "  the submodule's branch kept its commit" "$(git -C "$AL/s" rev-parse main)" "$S_NEW"
ok "  and the result says so" "$(printf '%s' "$OUT" | jqf detail)" "main still holds"
ok "  nothing mismatches any more" "$(printf '%s' "$OUT" | jqf tree.submodule_mismatch)" "[]"
REC="$(printf '%s' "$OUT" | jqf tree.recovery)"
ok "  its undo puts the submodule back on its branch" "$( (cd "$AL" && sh -c "$REC" >/dev/null 2>&1); echo "rc=$?"):$(git -C "$AL/s" symbolic-ref -q --short HEAD)" "rc=0:main"
git -C "$AL/s" checkout -q --detach "$S_OLD"
OUT=$(probe tree_sub_align "$AL" "s")
ok "a submodule already at the recorded commit is left alone" "$(printf '%s' "$OUT" | jqf headline)" "already at the commit this repository records"
OUT=$(probe tree_sub_align "$AL" "a.txt")
ok "a path that is no submodule is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "not-a-submodule"
OUT=$(probe tree_sub_align "$AL" "s,a.txt")
ok "one already aligned and one refused is a refusal, not a green \"every one is aligned\"" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf refusal.message)" "False:a.txt isn't a submodule of this repository. Already at the recorded commit: s."
# Update to <recorded>: checked out whatever the configuration says, never over what it would lose.
git -C "$AL" reset -q --hard "$BUMP"
git -C "$AL" config submodule.s.update none
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$AL\",\"paths\":[\"s\"]}")
ok "Update to <recorded> checks it out even where submodule.<name>.update says none" "$(printf '%s' "$OUT" | jqf value.ok):$(git -C "$AL/s" rev-parse HEAD)" "True:$S_NEW"
git -C "$AL" config --unset submodule.s.update
git -C "$AL/s" checkout -q --detach "$S_OLD"; git -C "$AL/s" commit -q --allow-empty -m "loose again"; LOOSE2="$(git -C "$AL/s" rev-parse HEAD)"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$AL\",\"paths\":[\"s\"]}")
ok "Update to over a commit only a detached HEAD holds is refused" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "submodule-detached-commits"
ok "  and nothing moved" "$(git -C "$AL/s" rev-parse HEAD)" "$LOOSE2"
git -C "$AL/s" checkout -q --detach "$S_OLD"; printf 'edit\n' >> "$AL/s/file.txt"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$AL\",\"paths\":[\"s\"]}")
ok "Update to over changes inside is refused" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "submodule-changes"
ok "  and the change is still there" "$(git -C "$AL/s" status --porcelain)" " M file.txt"
git -C "$AL/s" checkout -q -- file.txt
# A nested submodule's loose commit stops it too: --recursive moves nested ones.
git -C "$AL/s" switch -q main 2>/dev/null
git -C "$AL/s" submodule add -q "$SB/orphan.git" n 2>/dev/null; git -C "$AL/s" commit -qm "s: add n"
git -C "$AL/s/n" switch -q --detach; git -C "$AL/s/n" commit -q --allow-empty -m "loose in n"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$AL\",\"paths\":[\"s\"]}")
ok "Update to over a commit only a nested submodule's detached HEAD holds is refused" "$(printf '%s' "$OUT" | jqf value.refusal.code):$(printf '%s' "$OUT" | jqf value.refusal.message)" "submodule-detached-commits:1 commit(s) at s/n's checkout"
git -C "$AL/s/n" switch -q main 2>/dev/null
# A nested record its submodule never fetched (s moved on, n did not): nothing of n would be lost.
git -C "$AL/s" update-index --cacheinfo "160000,1111111111111111111111111111111111111111,n"
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$AL\",\"paths\":[\"s\"]}")
ok "a nested record its submodule has not fetched does not make Update to refuse" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print("refused: " + v["refusal"]["message"] if v.get("refusal") else "ran")')" "ran"

section "A backup branch is marked as one, and never named as containing a branch"
git init -q "$SB/bk"; printf '1\n' > "$SB/bk/f"; git -C "$SB/bk" add -A >/dev/null; git -C "$SB/bk" commit -qm one
printf '2\n' > "$SB/bk/f"; git -C "$SB/bk" commit -qam two
git -C "$SB/bk" branch topic HEAD~1
probe tree_reset "$SB/bk" "HEAD~1,hard" >/dev/null
BK_NAME="$(git -C "$SB/bk" for-each-ref --format='%(refname:short)' 'refs/heads/gitswitch-before-reset-*' | head -1)"
ok "(the reset kept a backup branch)" "$BK_NAME" "gitswitch-before-reset-"
OUT=$(probe branches "$SB/bk")
ok "the branch list marks it as a backup, and only it" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(sorted((b["name"].split("-20")[0], b["is_backup"]) for b in json.load(sys.stdin) if not b["is_remote"]))')" "[('gitswitch-before-reset', True), ('main', False), ('topic', False)]"
OUT=$(probe merge_info "$SB/bk" "topic")
ok "a branch's \"already contained in\" never names a backup" "$(printf '%s' "$OUT" | jqf merged_into)" '["main"]'

section "History's graph: lanes carry on across pages, a filtered list joins what it shows, and how a branch got in"
HG="$SB/hgraph"; git init -q "$HG"
for i in $(seq 1 60); do
  if [ $((i % 2)) -eq 0 ]; then who="Other <other@example.test>"; else who="Me <me@example.test>"; fi
  git -C "$HG" -c user.name="${who%% <*}" -c user.email="$(printf '%s' "$who" | sed 's/.*<\(.*\)>/\1/')" commit -q --allow-empty -m "c$i"
done
OUT=$(pinvoke history_page "{\"repoPath\":\"$HG\",\"rev\":\"HEAD\",\"offset\":50,\"limit\":50}")
ok "page 2 carries the line on from page 1: its first row is no tip" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;c=json.load(sys.stdin)["value"]["commits"];print(len(c), c[0].get("from_above"), c[0]["lane"])')" "10 True 0"
OUT=$(pinvoke history_page "{\"repoPath\":\"$HG\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":50}")
ok "  and the newest commit is one" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["commits"][0].get("from_above"))')" "False"
OUT=$(pinvoke history_page "{\"repoPath\":\"$HG\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":50,\"authors\":[\"me@example.test\"]}")
ok "a list filtered by a person joins only the commits it shows: one lane, not one more per row" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["total"], v["max_lane"])')" "30 0"
OUT=$(pinvoke history_page "{\"repoPath\":\"$HG\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":50,\"search\":\"c1\"}")
ok "  and so does one filtered by words" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["total"], v["max_lane"])')" "11 0"
# How a branch got into main: a fast-forward, then a merge commit.
git -C "$HG" switch -q -c ff; git -C "$HG" commit -q --allow-empty -m "ff work"; git -C "$HG" switch -q main 2>/dev/null || git -C "$HG" switch -q master
git -C "$HG" merge -q --ff-only ff
git -C "$HG" switch -q -c topic HEAD~3; git -C "$HG" commit -q --allow-empty -m "topic work"; git -C "$HG" switch -q -
git -C "$HG" merge -q --no-ff -m "merge topic" topic
OUT=$(pinvoke history_branch_merges "{\"repoPath\":\"$HG\",\"branch\":\"topic\"}")
ok "a branch merged with a merge commit names that commit" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print([(m["how"], (m["commit"] or {}).get("subject")) for m in v["merged_by"]])')" "('merge', 'merge topic')"
OUT=$(pinvoke history_branch_merges "{\"repoPath\":\"$HG\",\"branch\":\"ff\"}")
ok "  one taken in by a fast-forward says so" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print([m["how"] for m in v["merged_by"]])')" "fast-forward"
OUT=$(pinvoke history_branch_merges "{\"repoPath\":\"$HG\",\"branch\":\"HEAD\"}")
ok "  and the merges are counted, not capped silently" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["merges_total"], v["merges_truncated"])')" "1 False"
# A range hides commits too: its graph joins only what it lists.
R2="$SB/hrange"; git init -q "$R2"
git -C "$R2" commit -q --allow-empty -m c1; git -C "$R2" commit -q --allow-empty -m c2
git -C "$R2" switch -q -c side HEAD~1; git -C "$R2" commit -q --allow-empty -m s1; git -C "$R2" switch -q -
git -C "$R2" merge -q --no-ff -m m1 side
OUT=$(pinvoke history_page "{\"repoPath\":\"$R2\",\"rev\":\"HEAD~1..HEAD\",\"offset\":0,\"limit\":50}")
ok "a range joins only the commits it lists: no lane runs off to one it leaves out" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["total"], v["max_lane"])')" "2 0"

verify_result
