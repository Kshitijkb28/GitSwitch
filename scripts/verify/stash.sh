#!/bin/bash
# Stash suite, against real git: push (never --all), list/show/diff, apply vs
# pop, a pop that conflicts, restore one file, drop with its undo command,
# stale indexes, and what is refused while a merge is in progress.
# Throwaway repos, an isolated HOME, local bare repos as the "remotes".
set -u
source "$(dirname "$0")/lib.sh"
verify_init stash "Stash Tester" "stash@example.test"
SB="$(realdir "$SB")"

W="$SB/work"
count_stashes() { git -C "$W" stash list | wc -l | tr -d ' '; }
porcelain() { git -C "$W" status --porcelain | tr '\n' '|'; }
blob_in_index() { git -C "$W" rev-parse --verify --quiet ":0:$1"; }
blob_in_stash() { git -C "$W" rev-parse --verify --quiet "$1:$2"; }
# Back to a clean tree without touching ignored files or the submodule's inside.
tidy() { git -C "$W" reset -q --hard; git -C "$W" clean -fdq -e .env; }

# --- fixture: bare remote, a clone with two commits, .env ignored, a submodule --
git init -q --bare "$SB/remote.git"
git clone -q "$SB/remote.git" "$W" 2>/dev/null
printf 'line1\nline2\nline3\n' > "$W/a.txt"; printf 'b1\nb2\n' > "$W/b.txt"; printf '.env\n' > "$W/.gitignore"
git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "initial"
printf 'c\n' > "$W/c.txt"; git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "second"
git -C "$W" push -q -u origin main 2>/dev/null
printf 'SECRET=1\n' > "$W/.env"
git init -q --bare "$SB/sub.git"
git clone -q "$SB/sub.git" "$SB/subwork" 2>/dev/null
printf 'sub\n' > "$SB/subwork/s.txt"; git -C "$SB/subwork" add -A >/dev/null; git -C "$SB/subwork" commit -qm "sub init"; git -C "$SB/subwork" push -q origin main 2>/dev/null
git -C "$W" -c protocol.file.allow=always submodule add -q "$SB/sub.git" vendor/sub 2>/dev/null
git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "add submodule" >/dev/null; git -C "$W" push -q origin main 2>/dev/null

section "Nothing to stash"
OUT=$(probe stash_push "$W" "")
ok "a clean tree is refused before git runs" "$(printf '%s' "$OUT" | jqf refusal.code)" "nothing-to-stash"
ok "  and says so in plain words" "$(printf '%s' "$OUT" | jqf refusal.message)" "working tree is clean"
printf 'u\n' > "$W/only-new.txt"
OUT=$(probe stash_push "$W" "")
ok "an untracked file alone is nothing to stash without asking for new files" "$(printf '%s' "$OUT" | jqf refusal.code)" "nothing-to-stash"
rm "$W/only-new.txt"
printf 'dirty inside\n' >> "$W/vendor/sub/s.txt"
OUT=$(probe stash_push "$W" "")
ok "a dirty submodule alone gets git's own 'nothing to stash', measured" "$(printf '%s' "$OUT" | jqf advice.headline)" "Nothing to stash."
ok "  and the submodule is named as the reason" "$(printf '%s' "$OUT" | jqf detail)" "vendor/sub"
ok "  nothing was created" "$(count_stashes)" "0"

section "Push: staged, unstaged and untracked, never --all"
printf 'line1\nline2 edited\nline3\n' > "$W/a.txt"; git -C "$W" add a.txt
printf 'b3\n' >> "$W/b.txt"
printf 'new\n' > "$W/new.txt"
OUT=$(probe stash_push "$W" "fix header,untracked")
ok "the stash is made" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  headline counts what the stash holds" "$(printf '%s' "$OUT" | jqf headline)" "Stashed 3 changes."
ok "  the new entry is stash@{0}" "$(printf '%s' "$OUT" | jqf stash.entry.ref)" "stash@{0}"
ok "  with the whole subject git wrote" "$(printf '%s' "$OUT" | jqf stash.entry.message)" "On main: fix header"
ok "  with the undo command" "$(printf '%s' "$OUT" | jqf stash.recovery)" "git stash pop 'stash@{0}'"
ok "  the tree is clean apart from the submodule" "$(porcelain)" " M vendor/sub|"
ok "  the ignored .env is still there" "$([ -e "$W/.env" ] && echo kept || echo GONE)" "kept"
ok "  and still ignored" "$(git -C "$W" check-ignore .env)" ".env"
ok "  the dirty submodule is named as not stashed" "$(printf '%s' "$OUT" | jqf stash.not_stashed_submodules)" "vendor/sub"
ok "  in words too" "$(printf '%s' "$OUT" | jqf detail)" "a stash never includes submodules"
ok "  git agrees the subject is what we said" "$(git -C "$W" stash list --format=%gs)" "On main: fix header"

section "List, show, diff"
OUT=$(probe stash_list "$W")
ok "one entry" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(len(json.load(sys.stdin)))')" "1"
E=$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.dumps(json.load(sys.stdin)[0]))')
ok "  on branch main" "$(printf '%s' "$E" | jqf branch)" "main"
ok "  message without the prefix" "$(printf '%s' "$E" | jqf message)" "fix header"
ok "  two tracked files" "$(printf '%s' "$E" | jqf tracked_files)" "2"
ok "  one untracked file" "$(printf '%s' "$E" | jqf untracked_files)" "1"
ok "  not an autostash" "$(printf '%s' "$E" | jqf is_autostash)" "False"
ok "  the date is ISO 8601" "$(printf '%s' "$E" | jqf date | cut -c5)" "-"
OUT=$(probe stash_show "$W" "0")
FILES=$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print("|".join(f["status"]+" "+f["path"] for f in json.load(sys.stdin)["files"]))')
ok "show lists the modified files" "$FILES" "M a.txt|M b.txt"
ok "  and the new file as untracked" "$FILES" "untracked new.txt"
ok "  with line counts" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([(f["added"],f["removed"]) for f in json.load(sys.stdin)["files"] if f["path"]=="b.txt"][0])')" "(1, 0)"
OUT=$(probe stash_diff "$W" "0,a.txt")
ok "a tracked file's diff has a hunk" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([l["kind"] for l in json.load(sys.stdin)["lines"]])')" "'hunk'"
ok "  and the changed line" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([l["text"] for l in json.load(sys.stdin)["lines"] if l["kind"]=="add"])')" "+line2 edited"
OUT=$(probe stash_diff "$W" "0,new.txt")
ok "an untracked file's diff shows it added from nothing" "$(printf '%s' "$OUT" | jqf added)" "1"
ok "  as a new file" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(" ".join(l["text"] for l in json.load(sys.stdin)["lines"] if l["kind"]=="meta"))')" "new file mode"
OUT=$(probe stash_diff "$W" "0,c.txt")
ok "a file that isn't in the stash is an error, not an empty diff" "$OUT" "isn't in that stash"
OUT=$(probe stash_show "$W" "7")
ok "showing a stale index says the list is out of date" "$OUT" "isn't there any more"
printf 'x\n' >> "$W/c.txt"; git -C "$W" stash push -q -m autostash
OUT=$(probe stash_list "$W")
ok "an entry named by a rebase's autostash is flagged" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)[0]["is_autostash"])')" "True"
ok "  the hand-made one is not" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)[1]["is_autostash"])')" "False"
git -C "$W" stash pop -q; git -C "$W" checkout -q -- c.txt

section "Push only the selected files"
printf 'line1\nline2 only-a\nline3\n' > "$W/a.txt"; printf 'b-part\n' >> "$W/b.txt"
OUT=$(probe stash_push_paths "$W" "-,b.txt")
ok "a partial stash is made" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  b.txt left the tree, a.txt stayed" "$(porcelain)" " M a.txt| M vendor/sub|"
ok "  the result says a change is still in the tree" "$(printf '%s' "$OUT" | jqf detail)" "1 change is still in the tree"
OUT=$(probe stash_show "$W" "0")
ok "  and the stash holds b.txt" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print("|".join(f["path"] for f in json.load(sys.stdin)["files"]))')" "b.txt"
OUT=$(probe stash_push_paths "$W" "-,c.txt")
ok "selected files without changes: nothing is stashed and it says so" "$(printf '%s' "$OUT" | jqf advice.headline)" "Nothing to stash."
ok "  in words" "$(printf '%s' "$OUT" | jqf detail)" "None of the selected files"
ok "  count unchanged" "$(count_stashes)" "2"
git -C "$W" stash drop -q 'stash@{0}'; tidy

section "Apply keeps the entry, pop removes it"
OUT=$(probe stash_apply "$W" "0")
ok "apply brings the files back" "$(printf '%s' "$OUT" | jqf headline)" "Applied stash@{0}."
ok "  all three, unstaged (no --index)" "$(porcelain)" " M a.txt| M b.txt| M vendor/sub|?? new.txt|"
ok "  the entry is kept" "$(printf '%s' "$OUT" | jqf stash.kept)" "True"
ok "  stash count unchanged" "$(printf '%s' "$OUT" | jqf stash_count)" "1"
ok "  and the detail says so" "$(printf '%s' "$OUT" | jqf detail)" "still in the list"
OUT=$(probe stash_apply "$W" "0,pop")
ok "popping onto the same dirty files is refused by git and explained" "$(printf '%s' "$OUT" | jqf advice.action)" "stash-first"
ok "  the files in the way are named" "$(printf '%s' "$OUT" | jqf advice.guidance)" "a.txt"
ok "  the entry is still there" "$(count_stashes)" "1"
tidy
OUT=$(probe stash_apply "$W" "0,index")
ok "apply --index restores the staged/unstaged split" "$(porcelain)" "M  a.txt| M b.txt| M vendor/sub|?? new.txt|"
ok "  and says so" "$(printf '%s' "$OUT" | jqf detail)" "split restored"
tidy
OUT=$(probe stash_apply "$W" "0,pop")
ok "pop brings the files back" "$(printf '%s' "$OUT" | jqf headline)" "Popped stash@{0}."
ok "  and removes the entry" "$(printf '%s' "$OUT" | jqf stash_count)" "0"
ok "  git agrees" "$(count_stashes)" "0"
ok "  kept is false" "$(printf '%s' "$OUT" | jqf stash.kept)" "False"

section "A pop that conflicts keeps the entry"
OUT=$(probe stash_push "$W" "for conflict,untracked")
ok "a fresh stash of the same changes" "$(printf '%s' "$OUT" | jqf stash_count)" "1"
STASH_OID=$(git -C "$W" rev-parse 'stash@{0}')
printf 'line1\nline2 committed\nline3\n' > "$W/a.txt"; git -C "$W" commit -qam "conflicting commit"
OUT=$(probe stash_apply "$W" "0,pop")
ok "the pop reports the conflict" "$(printf '%s' "$OUT" | jqf ok)" "False"
ok "  with resolution advice" "$(printf '%s' "$OUT" | jqf advice.action)" "resolve-conflicts"
ok "  the entry is kept" "$(printf '%s' "$OUT" | jqf stash.kept)" "True"
ok "  the conflicted file is named" "$(printf '%s' "$OUT" | jqf stash.conflicts)" "a.txt"
ok "  one conflict in the tree" "$(printf '%s' "$OUT" | jqf conflicted)" "1"
ok "  with no operation in progress" "$(printf '%s' "$OUT" | jqf operation)" "None"
ok "  and the status knows a stash caused it" "$(printf '%s' "$OUT" | jqf conflict_source)" "stash"
ok "  git still lists the entry" "$(git -C "$W" stash list --format=%gs)" "On main: for conflict"
ok "  same commit as before" "$(git -C "$W" rev-parse 'stash@{0}')" "$STASH_OID"
ok "  git's own words are quoted" "$(printf '%s' "$OUT" | jqf advice.git_said)" "The stash entry is kept"
OUT=$(probe stash_restore "$W" "0,a.txt")
ok "restoring the file from the stash resolves the conflict" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  it is no longer conflicted" "$(printf '%s' "$OUT" | jqf conflicted)" "0"
ok "  the index holds the stash's version" "$(blob_in_index a.txt)" "$(blob_in_stash 'stash@{0}' a.txt)"
ok "  and is staged" "$(git -C "$W" diff --cached --name-only)" "a.txt"
ok "  the detail says the conflict is resolved" "$(printf '%s' "$OUT" | jqf detail)" "conflict on it is resolved"
tidy
OUT=$(probe stash_apply "$W" "0")
ok "an apply that conflicts prints nothing with --quiet, so it is measured" "$(printf '%s' "$OUT" | jqf advice.action)" "resolve-conflicts"
ok "  conflicted file named" "$(printf '%s' "$OUT" | jqf stash.conflicts)" "a.txt"
OUT=$(probe discard "$W" "a.txt")
ok "discarding the conflicted file is allowed (no operation is in progress)" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and clears the conflict" "$(printf '%s' "$OUT" | jqf conflicted)" "0"
ok "  the stash is still listed" "$(count_stashes)" "1"
tidy

section "Restore one file from a stash"
printf 'line1\nline2 two-files\nline3\n' > "$W/a.txt"; printf 'b-two\n' >> "$W/b.txt"
OUT=$(probe stash_push "$W" "two files")
ok "a two-file stash" "$(printf '%s' "$OUT" | jqf headline)" "Stashed 2 changes."
OUT=$(probe stash_restore "$W" "0,b.txt")
ok "restores one of them" "$(printf '%s' "$OUT" | jqf headline)" "Restored b.txt from stash@{0}."
ok "  the file matches the stash blob" "$(blob_in_index b.txt)" "$(blob_in_stash 'stash@{0}' b.txt)"
ok "  and is staged; the other file is untouched" "$(porcelain)" "M  b.txt| M vendor/sub|"
ok "  the entry is kept" "$(count_stashes)" "2"
OUT=$(probe stash_restore "$W" "0,c.txt")
ok "a file not in the stash is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "not-in-stash"
ok "  naming the file" "$(printf '%s' "$OUT" | jqf refusal.message)" "c.txt isn't in that stash"
OUT=$(probe stash_restore "$W" "1,new.txt")
ok "an untracked file comes back from the stash's third parent" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  with the stashed content" "$(cat "$W/new.txt")" "new"
ok "  staged, so it shows as added" "$(git -C "$W" status --porcelain new.txt)" "A  new.txt"
OUT=$(probe stash_restore "$W" "0,../outside")
ok "a path outside the worktree is rejected" "$OUT" "isn't a path inside this repository"
tidy

section "Drop, with the way back"
OUT=$(probe stash_drop "$W" "0")
ok "drop works" "$(printf '%s' "$OUT" | jqf headline)" "Dropped stash@{0}."
ok "  count decreased" "$(printf '%s' "$OUT" | jqf stash_count)" "1"
ok "  git agrees" "$(count_stashes)" "1"
REC=$(printf '%s' "$OUT" | jqf stash.recovery)
ok "  the undo is a git stash store command" "$REC" "git stash store -m 'On main: two files'"
ok "  carrying the commit id" "$REC" "$(printf '%s' "$OUT" | jqf stash.entry.oid)"
ok "  and the detail explains the object survives" "$(printf '%s' "$OUT" | jqf detail)" "commit object survives"
(cd "$W" && eval "$REC")
ok "running the undo command brings the entry back" "$(count_stashes)" "2"
ok "  as the newest entry, with its subject" "$(git -C "$W" stash list --format=%gs | head -1)" "On main: two files"

section "Stale indexes"
OUT=$(probe stash_apply "$W" "7")
ok "applying an index that isn't there is refused by name" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-stash"
OUT=$(probe stash_drop "$W" "7")
ok "so is dropping it" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-stash"
OUT=$(probe stash_restore "$W" "7,a.txt")
ok "and restoring from it" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-stash"
ok "  nothing changed" "$(count_stashes)" "2"

section "Busy: a merge in progress"
git -C "$W" switch -q -c side; printf 'side\n' > "$W/a.txt"; git -C "$W" commit -qam "side"
git -C "$W" switch -q main; printf 'main\n' > "$W/a.txt"; git -C "$W" commit -qam "main side"
git -C "$W" merge side >/dev/null 2>&1
ok "the fixture is a conflicted merge" "$(porcelain)" "UU a.txt"
OUT=$(probe stash_push "$W" "during merge")
ok "push is refused while the merge is in progress" "$(printf '%s' "$OUT" | jqf refusal.code)" "operation-in-progress"
ok "  with the abort command" "$(printf '%s' "$OUT" | jqf refusal.message)" "git merge --abort"
OUT=$(probe stash_apply "$W" "0")
ok "apply is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "operation-in-progress"
OUT=$(probe stash_drop "$W" "0")
ok "drop is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "operation-in-progress"
OUT=$(probe stash_restore "$W" "0,b.txt")
ok "restoring one file is still allowed" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and the file matches the stash" "$(blob_in_index b.txt)" "$(blob_in_stash 'stash@{0}' b.txt)"
ok "  the merge is still in progress" "$(printf '%s' "$OUT" | jqf operation.kind)" "merge"
git -C "$W" merge --abort
ok "the merge can still be aborted" "$(porcelain)" " M vendor/sub|"
ok "  and every stash is still listed" "$(count_stashes)" "2"

verify_result
