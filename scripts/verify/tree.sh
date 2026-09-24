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
ok "  in words" "$(printf '%s' "$OUT" | jqf detail)" "Update submodules to align them"
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
ok "a hard reset on a dirty tree is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "dirty-tree"
ok "  HEAD did not move" "$(head_of "$W")" "$MERGE_SHA"
OUT=$(probe tree_reset "$W" "HEAD~1,hard,stash")
ok "with 'stash first' it succeeds" "$(printf '%s' "$OUT" | jqf headline)" "Reset to $(git -C "$W" rev-parse --short "$FIRST_PARENT") (hard)."
ok "  the dirt is in stash@{0}" "$(git -C "$W" stash list --format=%gs -1)" "gitswitch before reset"
ok "  the result names the stash" "$(printf '%s' "$OUT" | jqf tree.stash.ref)" "stash@{0}"
ok "  the tree is clean" "$(porcelain "$W")" "clean"
BK="$(printf '%s' "$OUT" | jqf tree.backup.branch)"
ok "  the backup holds the old HEAD" "$(git -C "$W" rev-parse "$BK")" "$MERGE_SHA"
ok "  and its recovery is a hard reset to it" "$(printf '%s' "$OUT" | jqf tree.backup.recovery)" "reset --hard $BK"
ok "  running it restores the merge" "$(run_recovery "$(printf '%s' "$OUT" | jqf tree.backup.recovery)")" "rc=0"
git -C "$W" stash pop -q
ok "  and the stash pops the dirt back" "$(git -C "$W" status --porcelain -- b.txt)" " M b.txt"
git -C "$W" checkout -q -- b.txt; git -C "$W" branch -q -D "$BK"
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
OUT=$(probe tree_resolve_side "$W" "mine,vendor/sub")
ok "is not resolved by taking a side" "$(printf '%s' "$OUT" | jqf refusal.code)" "submodule-conflict"
OUT=$(probe tree_resolve_side "$W" "mine")
ok "no paths is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-paths"
probe abort "$W" >/dev/null
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
printf 'mod\n' >> "$W/a.txt"
OUT=$(probe tree_discard_all "$W" "stash")
ok "with a stash first the change is kept" "$(printf '%s' "$OUT" | jqf tree.stash.ref)" "stash@{0}"
ok "  git has it in stash@{0}" "$(git -C "$W" stash show --name-only 'stash@{0}')" "a.txt"
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

verify_result
