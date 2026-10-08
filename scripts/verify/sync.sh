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
ok "every commit of mine carries its author" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(sorted(set(c['author_email'] for c in d['superproject']['own_commits'])))")" "['sync@example.test']"
ok "  and a date" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(all(c['date'][:4].isdigit() for c in d['superproject']['own_commits']+d['superproject']['incoming']))")" "True"
ok "  upstream's bump of A says it moves A and is only a pointer" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);c=[c for c in d['superproject']['incoming'] if c['subject']=='super: bump A'][0];print(c['touches'],c['only_pointers'])")" "['A'] True"
ok "  upstream's real work is not a pointer bump" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);c=[c for c in d['superproject']['incoming'] if c['subject']=='super: upstream work'][0];print(c['only_pointers'])")" "False"
ok "  the base is where both agreed" "$(printf '%s' "$OUT" | jqf superproject.base_short)" "$(git -C "$SB/local" merge-base HEAD origin/main | cut -c1-7)"
ok "  A's row lists its own commit and what comes in" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);a=[s for s in d['submodules'] if s['path']=='A'][0];print([c['subject'] for c in a['own_commits']],[c['subject'] for c in a['incoming_commits']])")" "['A: my commit'] ['A: upstream 2', 'A: upstream 1']"
ok "  the uncommitted files are named" "$(printf '%s' "$OUT" | jqf superproject.dirty_paths)" "README.md"
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
ok "  the stash blocker carries its code" "$(printf '%s' "$OUT" | jqf superproject.remedies)" "turn-on-stash"
ok "without stashing a dirty tree is a named blocker" "$(printf '%s' "$OUT" | jqf blockers)" "Turn on stashing"
ok "  and the plan cannot run" "$(printf '%s' "$OUT" | jqf can_run)" "False"

section "Run: my commits end up on top, in the superproject and in A"
OUT=$(probe sync_run "$SB/local")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and says what it did" "$(printf '%s' "$OUT" | jqf headline)" "Updated: 2 commits of yours on top of 3 new commits from origin/main."
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
ok "a backup branch holds the old superproject tip" "$(git -C "$SB/local" rev-parse "$(git -C "$SB/local" branch --list 'gitswitch-before-update-*' | tr -d ' *')")" "$SUPER_HEAD0"
ok "  and one in A holds its old tip" "$(git -C "$SB/local/A" rev-parse "$(git -C "$SB/local/A" branch --list 'gitswitch-before-update-*' | tr -d ' *')")" "$A_HEAD0"
ok "  the result lists them with recovery commands" "$(printf '%s' "$OUT" | jqf sync.backups)" "reset --hard gitswitch-before-update-"
ok "  no backup is named after the old word" "$(git -C "$SB/local" branch --list 'gitswitch-before-sync-*' | wc -l | tr -d ' '):$(git -C "$SB/local/A" branch --list 'gitswitch-before-sync-*' | wc -l | tr -d ' ')" "0:0"
ok "  every check passed" "$(printf '%s' "$OUT" | jqf sync.verified)" "\"dirty_paths_unchanged\": true"
ok "  the outcome says what was done to each submodule, in its own words" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(sorted((x['path'],x['action']) for x in d['sync']['submodules']))")" "[('A', 'rebased'), ('B', 'fast-forwarded')]"
OUT=$(probe sync_plan "$SB/local")
ok "assessing again finds nothing to do" "$(printf '%s' "$OUT" | jqf nothing_to_do)" "True"
ok "  in words" "$(printf '%s' "$OUT" | jqf summary)" "Nothing to update"

section "A file conflict pauses the sync; abort puts everything back"
restore
git -C "$SB/local/A" checkout -q -- A.txt     # a clean A, so abort can put it back
printf 'shared line 1\nshared line 2 (mine)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "my shared edit"
SUPER_HEAD1="$(head_of "$SB/local")"; A_HEAD1="$(head_of "$SB/local/A")"
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "the sync pauses" "$(printf '%s' "$OUT" | jqf headline)" "Update paused: conflicts to resolve."
ok "  naming the file" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "shared.txt"
ok "  in the superproject" "$(printf '%s' "$OUT" | jqf sync.needs_user.where_)" "superproject"
ok "  A was already rebased at its stop" "$(git -C "$SB/local/A" log --format=%s -1)" "A: my commit"
ok "  the status carries the paused sync" "$(probe status "$SB/local" | jqf sync.phase)" "rebasing"
ok "  and a push is refused while it is paused" "$(probe push "$SB/local" "" | jqf refusal.code)" "sync-in-progress"
ok "  so is a plain pull" "$(probe pull "$SB/local" "merge" | jqf refusal.code)" "sync-in-progress"
OUT=$(probe sync_abort "$SB/local")
ok "abort reports itself" "$(printf '%s' "$OUT" | jqf headline)" "Update aborted."
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
ok "  the status counts what is still unmerged where the pause is" "$(probe status "$SB/local" | jqf sync.unresolved)" "1"
ok "  and names the commit being replayed" "$(probe status "$SB/local" | jqf sync.needs_user.at_commit)" "$(cat "$(git -C "$SB/local" rev-parse --path-format=absolute --git-path rebase-merge/stopped-sha)")"
printf 'shared line 1\n<<<<<<< HEAD\nshared line 2 (upstream)\n=======\nshared line 2 (mine)\n>>>>>>> my shared edit\nshared line 3\n' > "$SB/local/shared.txt"
OUT=$(probe stage "$SB/local" "shared.txt")
ok "marking it resolved with the markers still in it is refused by name" "$(printf '%s' "$OUT" | jqf refusal.code)" "conflict-markers"
ok "  naming the line" "$(printf '%s' "$OUT" | jqf refusal.message)" "shared.txt still contains conflict markers (line 2)"
OUT=$(probe stage_anyway "$SB/local" "shared.txt")
ok "  'anyway' stages it as it is" "$(printf '%s' "$OUT" | jqf ok)" "True"
OUT=$(probe sync_continue "$SB/local")
ok "  and continuing then refuses the markers that would be committed" "$(printf '%s' "$OUT" | jqf refusal.code)" "conflict-markers"
printf 'shared line 1\nshared line 2 (both)\nshared line 3\n' > "$SB/local/shared.txt"
OUT=$(probe stage "$SB/local" "shared.txt")
ok "staging the real resolution is allowed while paused" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  nothing is left unmerged" "$(probe status "$SB/local" | jqf sync.unresolved)" "0"
OUT=$(probe sync_continue "$SB/local")
ok "continue finishes the sync" "$(printf '%s' "$OUT" | jqf headline)" "Updated: 3 commits of yours on top of 3 new commits"
ok "  the resolved commit counts as on top" "$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "True"
ok "  so the result is the preview" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 3 commits of yours on top of 3 incoming."
ok "  my resolved commit is on top" "$(git -C "$SB/local" log --format=%s -1)" "my shared edit"
ok "  A and B are aligned" "$(git -C "$SB/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"
ok "  the README edit is back" "$(git -C "$SB/local" status --porcelain | grep -c 'README.md' || true)" "1"

section "A commit upstream already contains is dropped, and the result says so"
restore
printf 'shared line 1\nshared line 2 (upstream)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "same as upstream"
OUT=$(probe sync_plan "$SB/local")
ok "the plan foresees the drop" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([c['already_upstream'] for c in d['superproject']['own_commits'] if c['subject']=='same as upstream'])")" "[True]"
ok "  and only that one" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(sum(1 for c in d['superproject']['own_commits'] if c['already_upstream']))")" "1"
OUT=$(probe sync_run "$SB/local")
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and names the dropped commit" "$(printf '%s' "$OUT" | jqf detail)" "dropped because upstream already contained it: same as upstream"
ok "  the on-top check is measured, and passes" "$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "True"
ok "  so the result is the preview" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 2 commits of yours on top of 3 incoming."
ok "  the other two are on top" "$(subjects "$SB/local" origin/main..HEAD)" "local unrelated|seal work|"

section "A drop the preview did not foresee is still told apart from a loss"
restore
# Two commits of mine on one file: the first makes the change upstream also
# made, the second another one. The preview cannot foresee the first (once the
# second exists the file no longer equals upstream's); git finds it empty when
# it replays it and leaves it out, saying so.
printf 'shared line 1\nshared line 2 (upstream)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "same as upstream"
printf 'shared line 1\nshared line 2 (upstream)\nshared line 3\nshared line 4 (mine)\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "my line 4"
OUT=$(probe sync_plan "$SB/local")
ok "the plan does not foresee the drop" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([c['already_upstream'] for c in d['superproject']['own_commits'] if c['subject']=='same as upstream'])")" "[False]"
OUT=$(probe sync_run "$SB/local")
ok "the update succeeds all the same" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  the on-top check passes: git itself said the change was already upstream" "$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "True"
ok "  the dropped commit is named" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was dropped because upstream already contained it: same as upstream"
ok "  and the result says the preview did not show it" "$(printf '%s' "$OUT" | jqf detail)" "Not in the preview: git left out 1 commit of yours whose changes upstream already had"
ok "  so it does not claim to be as previewed" "$(printf '%s' "$OUT" | jqf detail | grep -c 'As previewed' || true)" "0"
ok "  the other three are on top" "$(subjects "$SB/local" origin/main..HEAD)" "my line 4|local unrelated|seal work|"

section "A commit thrown away by hand is a failed check, by name"
restore
printf 'shared line 1\nshared line 2 (mine)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "my shared edit"
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "paused on shared.txt" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "shared.txt"
git -C "$SB/local" rebase --skip >/dev/null 2>&1      # in a terminal: the commit being replayed is discarded
ok "  the rebase went on without it and finished" "$([ -d "$SB/local/.git/rebase-merge" ] && echo stopped || echo finished)" "finished"
OUT=$(probe sync_continue "$SB/local")
ok "the result is not a success" "$(printf '%s' "$OUT" | jqf ok)" "False"
ok "  the headline says a check failed" "$(printf '%s' "$OUT" | jqf headline)" "Update finished, but a check did not pass"
ok "  the on-top check is the one" "$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "False"
ok "  the missing commit is named, with where its original is" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours is missing after the replay and not found upstream: my shared edit. The originals are on the backup branch"
ok "  it is not called already upstream" "$(printf '%s' "$OUT" | jqf detail | grep -c 'already contained' || true)" "0"
ok "  the backup branch still holds it" "$(git -C "$SB/local" log --format=%s -1 "$(git -C "$SB/local" for-each-ref --format='%(refname:short)' 'refs/heads/gitswitch-before-update-*' | head -1)")" "my shared edit"

section "A submodule only my commits added is not called removed"
restore
( cd "$SB/local" && git submodule add -q "$SB/orphan.git" lib 2>/dev/null )
OUT=$(probe sync_plan "$SB/local")
ok "added but not committed yet: said so, with its own code" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=[s for s in d['submodules'] if s['path']=='lib'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked commit-first | it was added here but is not committed yet"
git -C "$SB/local" commit -qm "add lib"
OUT=$(probe sync_plan "$SB/local")
ok "committed: lib is no blocker, and the reason is the true one" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=[s for s in d['submodules'] if s['path']=='lib'][0];print(x['action'],'|',x['reason'])")" "none | added by your commits; upstream does not have it"
ok "  the update can run" "$(printf '%s' "$OUT" | jqf can_run)" "True"
git -C "$SB/local/lib" commit -q --allow-empty -m "lib: moved on"
ok "  its checkout moved off what my commit records: blocked, as any submodule would be" "$(probe sync_plan "$SB/local" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=[s for s in d['submodules'] if s['path']=='lib'][0];print(x['action'],x['remedy'])")" "blocked record-pointers"
git -C "$SB/local/lib" reset -q --hard HEAD~1
LIB0="$(head_of "$SB/local/lib")"
OUT=$(probe sync_run "$SB/local")
ok "the update succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  my commit adding it is on top" "$(git -C "$SB/local" log --format=%s -1)" "add lib"
ok "  it records the same commit and the checkout did not move" "$(git -C "$SB/local" rev-parse HEAD:lib):$(head_of "$SB/local/lib")" "$LIB0:$LIB0"
ok "  nothing is out of line" "$(git -C "$SB/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"

section "An untracked file inside a submodule, in the way of what comes in, is a blocker"
restore
printf 'mine\n' > "$SB/local/B/b2.txt"      # upstream's B adds b2.txt
OUT=$(probe sync_plan "$SB/local")
ok "B is blocked by name" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);b=[s for s in d['submodules'] if s['path']=='B'][0];print(b['action'],b['remedy'])")" "blocked untracked-inside"
ok "  the sentence names the file" "$(printf '%s' "$OUT" | jqf blockers)" "B: untracked file inside it would be overwritten by the commits it will be moved onto: b2.txt"
ok "  running anyway is refused, so nothing is left half-done" "$(probe sync_run "$SB/local" | jqf refusal.code)" "sync-blocked"
ok "  B did not move" "$(git -C "$SB/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"
rm "$SB/local/B/b2.txt"
ok "  with the file out of the way the update can run" "$(probe sync_plan "$SB/local" | jqf can_run)" "True"

section "A stash entry named autostash from an earlier rebase is not this run's"
restore
git -C "$SB/local" stash store -m autostash "$(git -C "$SB/local" stash create)"      # what a failed re-apply leaves behind
git -C "$SB/local" checkout -q -- README.md; git -C "$SB/local/A" checkout -q -- A.txt
ok "the old entry is there and the tree is clean" "$(git -C "$SB/local" stash list --format=%gs):$(git -C "$SB/local" status --porcelain --untracked-files=no --ignore-submodules=all | wc -l | tr -d ' ')" "autostash:0"
OUT=$(probe sync_run "$SB/local")
ok "the update does not mistake it for its own" "$(printf '%s' "$OUT" | jqf headline)" "Updated: 2 commits of yours on top of 3 new commits"
ok "  no stash is reported left behind" "$(printf '%s' "$OUT" | jqf sync.stash_left)" "False"
ok "  and the entry is still theirs to drop" "$(git -C "$SB/local" stash list | wc -l | tr -d ' ')" "1"

section "Abort puts a re-attached branch back where it stood"
restore
git -C "$SB/local/A" checkout -q -- A.txt; git -C "$SB/local/A" checkout -q --detach       # clean and detached; main inside A still at my commit
printf 'shared line 1\nshared line 2 (mine)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "my shared edit"
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "paused on the file" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "shared.txt"
ok "  after A was rebased and re-attached to main" "$(on_branch "$SB/local/A")" "main"
ok "  which moved main inside A onto the rebased commits" "$([ "$(git -C "$SB/local/A" rev-parse main)" != "$A_HEAD0" ] && echo moved || echo still)" "moved"
ok "  the submodule the run has not reached yet is not given a verb" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([s['action'] for s in d['sync']['submodules'] if s['path']=='B'])")" "['pending']"
OUT=$(probe sync_abort "$SB/local")
ok "abort detaches A where it was" "$(on_branch "$SB/local/A"):$(head_of "$SB/local/A")" "DETACHED:$A_HEAD0"
ok "  and puts main inside it back where it stood" "$(git -C "$SB/local/A" rev-parse main)" "$A_HEAD0"
ok "  the outcome calls A put back, and B untouched" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "[('A', 'put-back'), ('B', 'unchanged')]"

section "An update aborted in a terminal still puts back what it had moved"
restore
git -C "$SB/local/A" checkout -q -- A.txt
printf 'shared line 1\nshared line 2 (mine)\nshared line 3\n' > "$SB/local/shared.txt"; git -C "$SB/local" add shared.txt; git -C "$SB/local" commit -qm "my shared edit"
SUPER1="$(head_of "$SB/local")"
probe sync_plan "$SB/local" >/dev/null
OUT=$(probe sync_run "$SB/local")
ok "paused on the file, with A already rebased" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths):$([ "$(head_of "$SB/local/A")" != "$A_HEAD0" ] && echo rebased || echo not)" 'shared.txt"]:rebased'
git -C "$SB/local" rebase --abort >/dev/null 2>&1        # in a terminal
OUT=$(probe sync_continue "$SB/local")
ok "Continue update sees it was aborted" "$(printf '%s' "$OUT" | jqf headline)" "Update aborted."
ok "  and puts A back, saying so" "$(printf '%s' "$OUT" | jqf detail)" "A: put back at $(printf '%s' "$A_HEAD0" | cut -c1-7)"
ok "  A is where it was" "$(head_of "$SB/local/A"):$(on_branch "$SB/local/A")" "$A_HEAD0:main"
ok "  the superproject is where it was" "$(head_of "$SB/local")" "$SUPER1"
ok "  nothing is paused any more" "$(probe sync_abort "$SB/local" | jqf refusal.code)" "no-sync"
ok "  and the next preview is not blocked by leftovers" "$(probe sync_plan "$SB/local" | jqf can_run)" "True"

section "Blocked states are named, never guessed"
restore
git -C "$SB/local/A" commit -q --allow-empty -m "A: extra, unrecorded"
OUT=$(probe sync_plan "$SB/local")
ok "A checked out ahead of what my commit records is a blocker" "$(printf '%s' "$OUT" | jqf blockers)" "checked out at"
ok "  with a stable remedy code the page can key a button on" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([x['remedy'] for x in d['submodules'] if x['path']=='A'][0])")" "record-pointers"
ok "  running anyway is refused" "$(probe sync_run "$SB/local" | jqf refusal.code)" "sync-blocked"
restore
printf 'more\n' > "$SB/local/A/more.txt"; git -C "$SB/local/A" add -A >/dev/null; git -C "$SB/local/A" commit -qm "A: second"
git -C "$SB/local" add A >/dev/null; git -C "$SB/local" commit -qm "seal again"
OUT=$(probe sync_plan "$SB/local")
ok "two of my commits moving A along its line: A is rebased once, each commit recording its rewritten commit" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=[x for x in d['submodules'] if x['path']=='A'][0];print(x['action'],'|',' / '.join(x['notes']))")" "True:rebase | 2 commits of yours move its pointer; each will record the rewritten copy of the commit it pointed at"
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
ok "  the backup branch did NOT move with the rebase" "$(git -C "$SB/local" rev-parse "$(git -C "$SB/local" branch --list 'gitswitch-before-update-*' | tr -d ' *')")" "$SUPER_HEAD0"
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
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf headline)" "Updated: 1 commit of yours on top of 1 new commit"
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
ok "the sync succeeds" "$(printf '%s' "$OUT" | jqf headline)" "Updated: fast-forwarded to 1 new commit from origin/main."
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
# A dead address in the submodule's own config while .gitmodules still declares the live one
# (upstream moved nothing): the person's own setting is not overridden — blocked, with the way out named.
git -C "$SB/local/A" remote set-url origin "$SB/does-not-exist.git"
OUT=$(probe sync_plan "$SB/local")
ok "a submodule whose own address is dead is blocked, naming the address .gitmodules declares and git submodule sync as the way out" "$(printf '%s' "$OUT" | jqf blockers)" "it fetches from $SB/does-not-exist.git while .gitmodules declares $SB/A.git; \`git submodule sync\` re-points it"
ok "  and does not call it a move by upstream" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);a=[s for s in d['submodules'] if s['path']=='A'][0];print(a['url_change'], any('upstream moved it' in n for n in a['notes']))")" "None False"
restore
# A mirror of A chosen here, while upstream's declaration never changed: noted, left alone.
git clone -q --bare "$SB/A.git" "$SB/A-mirror.git" 2>/dev/null
git -C "$SB/local/A" remote set-url origin "$SB/A-mirror.git"
OUT=$(probe sync_plan "$SB/local")
ok "a mirror chosen here is not 'upstream moved it': the plan notes the difference and leaves it" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);a=[s for s in d['submodules'] if s['path']=='A'][0];print(a['url_change'], [n for n in a['notes'] if 'while .gitmodules declares' in n])")" "True:None ['it fetches from $SB/A-mirror.git while .gitmodules declares $SB/A.git; upstream did not move it, so the update leaves that as it is']"
OUT=$(probe sync_run "$SB/local")
ok "  and the run leaves the mirror in place" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$SB/local/A" remote get-url origin)" "True:$SB/A-mirror.git"
rm -rf "$SB/A-mirror.git"
restore
# Unreachable at the declared address too: that is the blocker.
mv "$SB/A.git" "$SB/A.git.away"
OUT=$(probe sync_plan "$SB/local")
ok "a submodule whose remote cannot be fetched, with a moving pointer, is blocked" "$(printf '%s' "$OUT" | jqf blockers)" "is not available here and fetching failed"
ok "  the plan still assessed B" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([x['action'] for x in d['submodules'] if x['path']=='B'])")" "['fast-forward']"
mv "$SB/A.git.away" "$SB/A.git"
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
OUT=$(probe sync_plan "$SB/local")
ok "the plan counts A's own commits, not the pointer commits" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([s['reason'] for s in d['submodules'] if s['path']=='A'][0])")" "your 2 commits are rebased onto"
OUT=$(probe sync_run "$SB/local")
ok "the sync pauses inside A" "$(printf '%s' "$OUT" | jqf headline)" "Update paused inside A."
ok "  saying where to resolve" "$(printf '%s' "$OUT" | jqf sync.needs_user.hint)" "Open it as its own repository"
ok "  A has the conflict" "$(git -C "$SB/local/A" status --porcelain | grep -c '^UU' || true)" "1"
T=$(git -C "$SB/local" rev-parse origin/main:A)
ok "  inside A, theirs is named after upstream's record" "$(probe status "$SB/local/A" | jqf operation.sides.theirs)" "origin/main's A ($(printf '%s' "$T" | cut -c1-7))"
ok "  A's status knows the update, its path and its target" "$(probe status "$SB/local/A" | jqf sync.sub_path):$(probe status "$SB/local/A" | jqf sync.sub_target)" "A:$T"
ok "  the superproject counts A's unresolved file — a conflict inside it, not a pointer to stage" "$(probe status "$SB/local" | jqf sync.unresolved):$(probe status "$SB/local" | jqf sync.handed_over)" "1:False"
OUT=$(probe sync_continue "$SB/local")
ok "  continuing before resolving is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-paths"
printf 'A v1\nline2 (both)\nline3\n' > "$SB/local/A/A.txt"; git -C "$SB/local/A" add A.txt
ok "  nothing is left unmerged" "$(probe status "$SB/local" | jqf sync.unresolved)" "0"
OUT=$(probe sync_continue "$SB/local")
ok "continue finishes the sync" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  the commit replayed with a rewritten pointer counts as on top" "$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "True"
ok "  A has my edits on top of upstream's" "$(git -C "$SB/local/A" log --format=%s -3 | tr '\n' '|')" "A: my edit of A.txt|A: my commit|A: upstream edits A.txt|"
ok "  and the superproject records the rebased A" "$(git -C "$SB/local" rev-parse HEAD:A)" "$(head_of "$SB/local/A")"
ok "  everything aligned" "$(git -C "$SB/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"

section "A plain pull says how many submodule pointers it moved"
git clone -q --recurse-submodules "$SB/super.git" "$SB/local3" 2>/dev/null
printf 'up3\n' > "$SB/upstream/A/up3.txt"; git -C "$SB/upstream/A" add -A >/dev/null; git -C "$SB/upstream/A" commit -qm "A: upstream 3"; git -C "$SB/upstream/A" push -q origin main 2>/dev/null
git -C "$SB/upstream" add A >/dev/null; git -C "$SB/upstream" commit -qm "super: bump A again"; git -C "$SB/upstream" push -q origin main 2>/dev/null
OUT=$(probe pull "$SB/local3" "ff-only")
ok "the pull fast-forwards" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and counts the submodule pointer it moved (A), not the one it did not (B)" "$(printf '%s' "$OUT" | jqf pull.submodules_moved)" "1"
ok "  the gitlink row says so too" "$(probe status "$SB/local3" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([e['sub_commit_changed'] for e in d['entries'] if e['path']=='A'])")" "[True]"

section "A fast-forward over an edited file is a blocker, not a failure at align time"
restore
printf 'B v2\nline2\nline3\n' > "$SB/upstream/B/B.txt"; git -C "$SB/upstream/B" commit -qam "B: upstream edits B.txt"; git -C "$SB/upstream/B" push -q origin main 2>/dev/null
git -C "$SB/upstream" add B >/dev/null; git -C "$SB/upstream" commit -qm "super: bump B again"; git -C "$SB/upstream" push -q origin main 2>/dev/null
printf 'local edit\n' >> "$SB/local/B/B.txt"
OUT=$(probe sync_plan "$SB/local")
ok "B is blocked by name" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);b=[s for s in d['submodules'] if s['path']=='B'][0];print(b['action'],b['remedy'])")" "blocked stash-inside"
ok "  the sentence names the file" "$(printf '%s' "$OUT" | jqf blockers)" "B: its uncommitted changes to B.txt collide with the commits it will be moved onto"
ok "  running anyway is refused" "$(probe sync_run "$SB/local" | jqf refusal.code)" "sync-blocked"

section "Identity: a rebase re-commits as whoever git is set to, so a mismatch blocks — a fast-forward does not"
restore
git -C "$SB/local/A" checkout -q -- A.txt   # upstream's A now edits A.txt too; that blocker is proven above
profile_json() { "$PY" - "$@" <<'PY'
import json, sys
f, name, gname, email, key, *dirs = sys.argv[1:]
p = {"name": name, "git_name": gname, "git_email": email, "directories": dirs}
if key: p["ssh_key_path"] = key
json.dump(p, open(f, "w"))
PY
}
profile_json "$SB/psync.json" "Sync" "Sync Tester" "sync@example.test" "" "$SB"
probe profile_apply "$SB" "$SB/psync.json" >/dev/null
git -C "$SB/local" config user.email wrong@example.test
OUT=$(probe sync_plan "$SB/local")
ok "with commits of mine to replay, the wrong identity is a blocker" "$(printf '%s' "$OUT" | jqf blockers)" "This folder should commit as sync@example.test but git is set to wrong@example.test"
ok "  with its code" "$(printf '%s' "$OUT" | jqf superproject.remedies)" "identity"
ok "  running anyway is refused" "$(probe sync_run "$SB/local" | jqf refusal.code)" "sync-blocked"
git -C "$SB/local" config --unset user.email
ok "  and the fix lifts it" "$(probe sync_plan "$SB/local" | jqf can_run)" "True"
git -C "$SB/local3" config user.email wrong@example.test
OUT=$(probe sync_plan "$SB/local3")
ok "with nothing of mine to replay and only fast-forwards, it is not" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf superproject.ahead)" "True:0"
git -C "$SB/local3" config --unset user.email

# --- worlds of their own -------------------------------------------------------
# A bare superproject and bare submodules seeded by an author, then `local`
# and `upstream` clones with every submodule on main.
mini() {
  local D="$SB/$1"; shift; mkdir -p "$D"
  git init -q --bare "$D/super.git"
  for s in "$@"; do
    git init -q --bare "$D/$s.git"; git clone -q "$D/$s.git" "$D/author-$s" 2>/dev/null
    printf '%s v1\nline2\nline3\n' "$s" > "$D/author-$s/$s.txt"
    git -C "$D/author-$s" add -A >/dev/null; git -C "$D/author-$s" commit -qm "$s: initial"; git -C "$D/author-$s" push -q origin main 2>/dev/null
  done
  git clone -q "$D/super.git" "$D/author" 2>/dev/null
  printf 'readme\n' > "$D/author/README.md"; git -C "$D/author" add -A >/dev/null; git -C "$D/author" commit -qm "super: initial"
  for s in "$@"; do ( cd "$D/author" && git submodule add -q "$D/$s.git" "$s" 2>/dev/null && git config -f .gitmodules "submodule.$s.branch" main ); done
  if [ "$#" -gt 0 ]; then git -C "$D/author" add -A >/dev/null; git -C "$D/author" commit -qm "super: add submodules"; fi
  git -C "$D/author" push -q origin main 2>/dev/null
  for c in local upstream; do
    git clone -q --recurse-submodules "$D/super.git" "$D/$c" 2>/dev/null
    for s in "$@"; do git -C "$D/$c/$s" checkout -q main; done
  done
}

section "Abort puts back a submodule the update only fast-forwarded"
mini ab B C
W="$SB/ab"
# Upstream: B moves forward; C's history is rewritten, so my C (still what my
# branch records) has diverged from what upstream records.
printf 'b2\n' > "$W/upstream/B/b2.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: upstream 1"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
printf 'C v1, rewritten\nline2\nline3\n' > "$W/upstream/C/C.txt"; git -C "$W/upstream/C" commit -q --amend -am "C: initial, rewritten"; git -C "$W/upstream/C" push -q -f origin main 2>/dev/null
git -C "$W/upstream" add B C >/dev/null; git -C "$W/upstream" commit -qm "super: bump B, rewrite C"; git -C "$W/upstream" push -q origin main 2>/dev/null
B0="$(head_of "$W/local/B")"; C0="$(head_of "$W/local/C")"; S0="$(head_of "$W/local")"
OUT=$(probe sync_plan "$W/local")
ok "the plan: B fast-forwards, C is rebased after the superproject" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action'],s['predicted_gitlink_conflict']) for s in d['submodules']])")" "[('B', 'fast-forward', False), ('C', 'rebase', False)]"
OUT=$(probe sync_run "$W/local")
ok "the update pauses inside C" "$(printf '%s' "$OUT" | jqf headline)" "Update paused inside C."
ok "  after B was fast-forwarded" "$([ "$(head_of "$W/local/B")" != "$B0" ] && echo moved || echo still)" "moved"
OUT=$(probe sync_abort "$W/local")
ok "abort says so" "$(printf '%s' "$OUT" | jqf headline)" "Update aborted."
ok "  and names B, which it put back too" "$(printf '%s' "$OUT" | jqf detail)" "B: put back at $(printf '%s' "$B0" | cut -c1-7)"
ok "  B is back where it was" "$(head_of "$W/local/B")" "$B0"
ok "  C is back where it was" "$(head_of "$W/local/C"):$(git -C "$W/local/C" status --porcelain | grep -c '^UU' || true)" "$C0:0"
ok "  the superproject is back where it was" "$(head_of "$W/local")" "$S0"
ok "  no submodule is left off its recorded commit" "$(git -C "$W/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"
ok "  the outcome calls both put back" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "[('B', 'put-back'), ('C', 'put-back')]"
ok "  and the next preview is not blocked by what the abort left" "$(probe sync_plan "$W/local" | jqf can_run)" "True"

section "A pointer handed over waits to be staged: the count says so, and Continue is refused until it is"
mini ho
W="$SB/ho"
git init -q --bare "$W/X.git"; git clone -q "$W/X.git" "$W/author-X" 2>/dev/null
printf 'x1\n' > "$W/author-X/x.txt"; git -C "$W/author-X" add -A >/dev/null; git -C "$W/author-X" commit -qm "X: one"
printf 'x2\n' >> "$W/author-X/x.txt"; git -C "$W/author-X" commit -qam "X: two"; git -C "$W/author-X" push -q origin main 2>/dev/null
X1="$(git -C "$W/author-X" rev-parse HEAD~1)"
# Both sides add X: upstream at its tip, I at the commit before.
( cd "$W/upstream" && git submodule add -q "$W/X.git" X 2>/dev/null ); git -C "$W/upstream" commit -qm "super: add X"; git -C "$W/upstream" push -q origin main 2>/dev/null
( cd "$W/local" && git submodule add -q "$W/X.git" X 2>/dev/null ); git -C "$W/local/X" checkout -q "$X1"; git -C "$W/local" add X; git -C "$W/local" commit -qm "add X here"
OUT=$(probe sync_plan "$W/local")
ok "the preview says the update will pause on X, not that it moves it" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['predicted_gitlink_conflict'],'|',x['reason'])")" "none True | added by your commits at"
OUT=$(probe sync_run "$W/local")
ok "the update hands X over" "$(printf '%s' "$OUT" | jqf sync.needs_user.where_)" "submodule:X"
CV=$(pinvoke changes_conflict_view "{\"repoPath\":\"$W/local\",\"path\":\"X\"}")
ok "  a pointer has no sides to show: the conflict view says what decides it" "$(printf '%s' "$CV" | jqf error)" "X is a submodule, not a file"
ok "  saying it has to be staged" "$(printf '%s' "$OUT" | jqf sync.needs_user.hint)" "decide by hand, stage it, then Continue update"
ok "  the status counts the pointer still to stage, and says it is a hand-over" "$(probe status "$W/local" | jqf sync.unresolved):$(probe status "$W/local" | jqf sync.handed_over)" "1:True"
OUT=$(probe sync_continue "$W/local")
ok "  Continue before staging is refused, not repeated" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-paths"
ok "    naming what to do" "$(printf '%s' "$OUT" | jqf refusal.message)" "X is still unmerged here. Put it where you want it and stage it"
git -C "$W/local" add X
ok "  staged, nothing is left" "$(probe status "$W/local" | jqf sync.unresolved):$(probe status "$W/local" | jqf sync.handed_over)" "0:False"
OUT=$(probe sync_continue "$W/local")
ok "then Continue finishes" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  with my commit on top, recording where I put X" "$(git -C "$W/local" log --format=%s -1):$(git -C "$W/local" rev-parse HEAD:X)" "add X here:$X1"

section "A pointer bump of mine that upstream has moved past is foreseen as dropped"
mini sp A
W="$SB/sp"
for n in one two three; do printf '%s\n' "$n" >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: $n"; done; git -C "$W/upstream/A" push -q origin main 2>/dev/null
ONE="$(git -C "$W/upstream/A" rev-parse HEAD~2)"; THREE="$(git -C "$W/upstream/A" rev-parse HEAD)"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: bump A to three"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only "$ONE"      # my A stops at one
git -C "$W/local" add A >/dev/null; git -C "$W/local" commit -qm "bump A to one"
OUT=$(probe sync_plan "$W/local")
ok "the preview marks my bump as one upstream already has" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(c['subject'],c['only_pointers'],c['already_upstream']) for c in d['superproject']['own_commits']])")" "[('bump A to one', True, True)]"
ok "  and A as a plain fast-forward" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([s['action'] for s in d['submodules']])")" "['fast-forward']"
OUT=$(probe sync_run "$W/local")
ok "the update succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  naming the bump it left out" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was dropped because upstream already contained it: bump A to one"
ok "  exactly as previewed" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 0 commits of yours on top of 1 incoming."
ok "  A is at upstream's commit and nothing is out of line" "$(head_of "$W/local/A"):$(git -C "$W/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "$THREE:0"

section "Two pointer commits of mine block only when upstream moved the pointer too"
mini sq A
W="$SB/sq"
printf 'u\n' > "$W/upstream/up.txt"; git -C "$W/upstream" add -A >/dev/null; git -C "$W/upstream" commit -qm "super: unrelated"; git -C "$W/upstream" push -q origin main 2>/dev/null
for n in one two; do printf '%s\n' "$n" >> "$W/local/A/A.txt"; git -C "$W/local/A" commit -qam "A: my $n"; git -C "$W/local" add A >/dev/null; git -C "$W/local" commit -qm "bump A to $n"; done
OUT=$(probe sync_plan "$W/local")
ok "upstream left the pointer alone: nothing to squash" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['action'],s['remedy']) for s in d['submodules']])")" "True:[('none', None)]"
OUT=$(probe sync_run "$W/local")
ok "the update replays both without a stop" "$(printf '%s' "$OUT" | jqf headline)" "Updated: 2 commits of yours on top of 1 new commit"
ok "  as previewed" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 2 commits of yours on top of 1 incoming."
ok "  both on top, in order" "$(subjects "$W/local" origin/main..HEAD)" "bump A to two|bump A to one|"
ok "  the branch records where A is" "$(git -C "$W/local" rev-parse HEAD:A)" "$(head_of "$W/local/A")"

section "Level with upstream, a submodule is aligned and the branch is not touched"
mini lv B
W="$SB/lv"
printf 'b2\n' > "$W/upstream/B/b2.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: upstream 1"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
B1="$(head_of "$W/upstream/B")"
git -C "$W/upstream" add B >/dev/null; git -C "$W/upstream" commit -qm "super: bump B"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null       # the branch records B's new commit; B's checkout has not followed
# (each file by name: `add -A` would also record B where its checkout still is)
git -C "$W/local" switch -q -c side; printf 's\n' > "$W/local/side.txt"; git -C "$W/local" add side.txt; git -C "$W/local" commit -qm "side work"
git -C "$W/local" switch -q main; printf 'm\n' > "$W/local/main.txt"; git -C "$W/local" add main.txt; git -C "$W/local" commit -qm "main work"; git -C "$W/local" merge -q --no-ff -m "merge side" side
printf 'edited\n' >> "$W/local/README.md"
S0="$(head_of "$W/local")"
OUT=$(probe sync_plan "$W/local" "nostash")
ok "only the submodule needs moving" "$(printf '%s' "$OUT" | jqf summary)" "Only submodules need aligning."
ok "  so my edits need no stash and nothing blocks" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf superproject.will_stash)" "True:False"
OUT=$(probe sync_run "$W/local" "nostash")
ok "the update says what it did" "$(printf '%s' "$OUT" | jqf headline)" "Submodules aligned: B. Your branch was already level with origin/main."
ok "  B is at the recorded commit" "$(head_of "$W/local/B")" "$B1"
ok "  the branch was not rewritten: same tip, my merge still a merge" "$(head_of "$W/local"):$(git -C "$W/local" rev-list --count --merges origin/main..HEAD)" "$S0:1"
ok "  my edit is still there" "$(git -C "$W/local" status --porcelain -- README.md)" " M README.md"

# A file with a few lines both sides can change in different places.
seed_file() {  # seed_file <world>: F.txt in the superproject, on both clones
  local D="$SB/$1"
  for i in 1 2 3 4 5 6 7 8 9 10 11 12; do printf 'line %s\n' "$i"; done > "$D/upstream/F.txt"
  git -C "$D/upstream" add F.txt; git -C "$D/upstream" commit -qm "super: F.txt"; git -C "$D/upstream" push -q origin main 2>/dev/null
  git -C "$D/local" pull -q --ff-only 2>/dev/null
}
edit_line() { sed -i.bak "s/^line $2\$/line $2 $3/" "$1"; rm -f "$1.bak"; }   # edit_line <file> <n> <suffix>

section "A commit of mine that moves a pointer back while upstream moved it forward is handed over, not undone"
mini rw A
W="$SB/rw"
for n in one two; do printf '%s\n' "$n" >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: $n"; done; git -C "$W/upstream/A" push -q origin main 2>/dev/null
ONE="$(git -C "$W/upstream/A" rev-parse HEAD~1)"; TWO="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A at two"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null; git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only "$TWO"
printf 'three\n' >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: three"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
THREE="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A to three"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local/A" reset -q --hard "$ONE"; git -C "$W/local" add A; git -C "$W/local" commit -qm "pin A back to one"
cp -R "$W/local" "$W/local2"
OUT=$(probe sync_plan "$W/local")
ok "the preview says the update will pause on the pointer" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['predicted_gitlink_conflict'],'|',x['reason'])")" "none True | your commit moves its pointer back to $(printf '%s' "$ONE" | cut -c1-7) while upstream moved it forward to $(printf '%s' "$THREE" | cut -c1-7)"
OUT=$(probe sync_run "$W/local")
ok "the update pauses and hands A over" "$(printf '%s' "$OUT" | jqf sync.needs_user.where_)" "submodule:A"
ok "  saying which is which" "$(printf '%s' "$OUT" | jqf sync.needs_user.hint)" "Your commit records A at $(printf '%s' "$ONE" | cut -c1-7), behind the $(printf '%s' "$THREE" | cut -c1-7) upstream moved it to"
ok "  A was not moved forward over what my commit says" "$(head_of "$W/local/A")" "$ONE"
ok "  the status calls it a hand-over with one pointer to stage" "$(probe status "$W/local" | jqf sync.handed_over):$(probe status "$W/local" | jqf sync.unresolved)" "True:1"
git -C "$W/local" add A        # leave it where I pinned it
OUT=$(probe sync_continue "$W/local")
ok "kept as I pinned it: the update finishes with my commit on top" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local" log --format=%s -1)" "True:pin A back to one"
ok "  recording my pointer, with A there" "$(git -C "$W/local" rev-parse HEAD:A):$(head_of "$W/local/A")" "$ONE:$ONE"
ok "  as the preview said" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 1 commit of yours on top of 1 incoming."
# The other choice, on a copy: take upstream's pointer — my commit then has nothing left to say.
probe sync_run "$W/local2" >/dev/null
git -C "$W/local2/A" merge -q --ff-only "$THREE"; git -C "$W/local2" add A
OUT=$(probe sync_continue "$W/local2")
ok "taking upstream's pointer instead: the update finishes" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local2" rev-parse HEAD:A)" "True:$THREE"
ok "  and the commit that was left with nothing is named, not passed over" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was left out: nothing of it remained once its conflict was resolved — pin A back to one."
ok "  without claiming the result is as previewed" "$(printf '%s' "$OUT" | jqf detail | grep -c 'As previewed' || true)" "0"

section "A pointer change that is staged and not committed blocks the update"
mini sg A
W="$SB/sg"
printf 'one\n' >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: one"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A at one"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null; git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only origin/main
printf 'u\n' > "$W/upstream/up.txt"; git -C "$W/upstream" add up.txt; git -C "$W/upstream" commit -qm "super: unrelated"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local/A" reset -q --hard HEAD~1; git -C "$W/local" add A       # staged, not committed
OUT=$(probe sync_plan "$W/local")
ok "the staged pointer is a blocker with its own sentence" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked commit-first | its pointer is staged at"
ok "  it is not called something my commits did" "$(printf '%s' "$OUT" | jqf blockers | grep -c 'your commits put' || true)" "0"
git -C "$W/local" restore --staged A; git -C "$W/local/A" merge -q --ff-only origin/main
ok "  unstaged and back in place, the update can run" "$(probe sync_plan "$W/local" | jqf can_run)" "True"

section "Untracked files in the way are found whatever git calls the change and however many there are"
mini rn B
W="$SB/rn"
git -C "$W/upstream/B" mv B.txt NOTES.txt; git -C "$W/upstream/B" commit -qm "B: rename"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
git -C "$W/upstream" add B >/dev/null; git -C "$W/upstream" commit -qm "super: bump B"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'mine\n' > "$W/local/B/NOTES.txt"
OUT=$(probe sync_plan "$W/local")
ok "a file upstream renamed onto an untracked one of mine is a blocker" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'])"):$(printf '%s' "$OUT" | jqf blockers)" "blocked untracked-inside:"
ok "  naming it" "$(printf '%s' "$OUT" | jqf blockers)" "NOTES.txt"
rm "$W/local/B/NOTES.txt"
printf 'z\n' > "$W/upstream/B/zz-new.txt"; git -C "$W/upstream/B" add zz-new.txt; git -C "$W/upstream/B" commit -qm "B: zz-new"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
git -C "$W/upstream" add B >/dev/null; git -C "$W/upstream" commit -qm "super: bump B again"; git -C "$W/upstream" push -q origin main 2>/dev/null
mkdir -p "$W/local/B/out"; i=0; while [ $i -lt 2010 ]; do : > "$W/local/B/out/f$i"; i=$((i+1)); done
printf 'mine\n' > "$W/local/B/zz-new.txt"
OUT=$(probe sync_plan "$W/local")
ok "with more untracked files than a status lists, the one in the way is still found" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'])"):$(printf '%s' "$OUT" | jqf blockers)" "blocked untracked-inside:"
ok "  by name" "$(printf '%s' "$OUT" | jqf blockers)" "zz-new.txt"

section "Level with upstream, a submodule's rebase finished in a terminal is carried on, not called an abort"
mini lt B C
W="$SB/lt"
printf 'b2\n' > "$W/upstream/B/b2.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: upstream 1"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
printf 'C v1, rewritten\nline2\nline3\n' > "$W/upstream/C/C.txt"; git -C "$W/upstream/C" commit -q --amend -am "C: initial, rewritten"; git -C "$W/upstream/C" push -q -f origin main 2>/dev/null
git -C "$W/upstream" add B C >/dev/null; git -C "$W/upstream" commit -qm "super: bump B, rewrite C"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null          # the branch is level; B and C have not followed
S0="$(head_of "$W/local")"
OUT=$(probe sync_run "$W/local")
ok "the update pauses inside C with the branch untouched" "$(printf '%s' "$OUT" | jqf headline):$([ "$(head_of "$W/local")" = "$S0" ] && echo same)" "Update paused inside C.:same"
printf 'C v1, both\nline2\nline3\n' > "$W/local/C/C.txt"; git -C "$W/local/C" add C.txt; GIT_EDITOR=true git -C "$W/local/C" rebase --continue >/dev/null 2>&1      # in a terminal
OUT=$(probe sync_continue "$W/local")
ok "Continue update carries on" "$(printf '%s' "$OUT" | jqf headline)" "Submodules aligned: B, C."
ok "  it is not called aborted, and nothing is left paused" "$(printf '%s' "$OUT" | jqf sync.phase):$(probe status "$W/local" | jqf sync)" "done:None"
ok "  every submodule matches what the branch records, or sits on top of it" "$(printf '%s' "$OUT" | jqf sync.verified.submodules_aligned)" "True"

section "More commits of mine than the preview lists are all counted, and a clean replay is not called a loss"
mini big
W="$SB/big"; seed_file big
edit_line "$W/local/F.txt" 5 "(mine)"; git -C "$W/local" commit -qam "my edit of l5"
i=1; while [ $i -le 34 ]; do printf '%s\n' "$i" > "$W/local/f$i.txt"; git -C "$W/local" add "f$i.txt"; git -C "$W/local" commit -qm "c$i"; i=$((i+1)); done
edit_line "$W/upstream/F.txt" 8 "(upstream)"; git -C "$W/upstream" commit -qam "super: l8"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "35 commits of mine, 30 of them listed" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin)['superproject'];print(d['ahead'],len(d['own_commits']),d['more_own'])")" "35 30 5"
OUT=$(probe sync_run "$W/local")
ok "the update succeeds" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "True:True"
ok "  and counts all of them" "$(printf '%s' "$OUT" | jqf headline)" "Updated: 35 commits of yours on top of 1 new commit"
ok "  as previewed" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 35 commits of yours on top of 1 incoming."
ok "  git agrees" "$(git -C "$W/local" rev-list --count origin/main..HEAD)" "35"

section "A lost commit is not excused by another with the same subject"
mini dup
W="$SB/dup"
printf 'readme (upstream)\n' > "$W/upstream/README.md"; git -C "$W/upstream" commit -qam "super: readme"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'a\n' > "$W/local/a.txt"; git -C "$W/local" add a.txt; git -C "$W/local" commit -qm "wip"
printf 'readme (mine)\n' > "$W/local/README.md"; git -C "$W/local" commit -qam "wip"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "paused on the README" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "README.md"
git -C "$W/local" rebase --skip >/dev/null 2>&1
OUT=$(probe sync_continue "$W/local")
ok "one of two commits called wip is gone: a failed check" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "False:False"
ok "  named as missing" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours is missing after the replay and not found upstream: wip."

section "A commit resolved into nothing is named, not passed over"
mini tu
W="$SB/tu"
printf 'readme (upstream)\n' > "$W/upstream/README.md"; git -C "$W/upstream" commit -qam "super: readme"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'a\n' > "$W/local/a.txt"; git -C "$W/local" add a.txt; git -C "$W/local" commit -qm "add a"
printf 'readme (mine)\n' > "$W/local/README.md"; git -C "$W/local" commit -qam "edit readme"
probe sync_plan "$W/local" >/dev/null
probe sync_run "$W/local" >/dev/null
probe tree_resolve_side "$W/local" "theirs,README.md" >/dev/null        # Take theirs: upstream's version
OUT=$(probe sync_continue "$W/local")
ok "the update finishes" "$(printf '%s' "$OUT" | jqf headline)" "Updated: 1 commit of yours on top of 1 new commit"
ok "  naming the commit that was left with nothing" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was left out: nothing of it remained once its conflict was resolved — edit readme."
ok "  and not claiming the result is as previewed" "$(printf '%s' "$OUT" | jqf detail | grep -c 'As previewed' || true)" "0"

section "A drop git made while the rebase was finished in a terminal is still told apart from a loss"
mini tc
W="$SB/tc"; seed_file tc
printf 'readme (upstream)\n' > "$W/upstream/README.md"; edit_line "$W/upstream/F.txt" 2 "CHANGED"; edit_line "$W/upstream/F.txt" 6 "(upstream)"
git -C "$W/upstream" commit -qam "super: big"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'readme (mine)\n' > "$W/local/README.md"; git -C "$W/local" commit -qam "X: readme"
edit_line "$W/local/F.txt" 2 "CHANGED"; git -C "$W/local" commit -qam "A: change l2"
edit_line "$W/local/F.txt" 11 "(mine)"; git -C "$W/local" commit -qam "B: change l11"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "paused on the README" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "README.md"
printf 'readme (mine)\n' > "$W/local/README.md"; git -C "$W/local" add README.md; GIT_EDITOR=true git -C "$W/local" rebase --continue >/dev/null 2>&1       # in a terminal
OUT=$(probe sync_continue "$W/local")
ok "the update is a success: what git left out was already upstream" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf sync.verified.own_on_top)" "True:True"
ok "  and it is named" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was dropped because upstream already contained it: A: change l2"
ok "  the other two are on top" "$(subjects "$W/local" origin/main..HEAD)" "B: change l11|X: readme|"

section "Abort calls a submodule put back when aborting its rebase already did that"
mini ae C
W="$SB/ae"
printf 'C v1\nline2 (upstream)\nline3\n' > "$W/upstream/C/C.txt"; git -C "$W/upstream/C" commit -qam "C: upstream edit"; git -C "$W/upstream/C" push -q origin main 2>/dev/null
git -C "$W/upstream" add C >/dev/null; git -C "$W/upstream" commit -qm "super: bump C"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'C v1\nline2 (mine)\nline3\n' > "$W/local/C/C.txt"; printf 'o\n' > "$W/local/C/other.txt"; git -C "$W/local/C" add -A >/dev/null; git -C "$W/local/C" commit -qm "C: my edit"
git -C "$W/local" add C >/dev/null; git -C "$W/local" commit -qm "seal C"
printf 'still editing\n' >> "$W/local/C/other.txt"
C0="$(head_of "$W/local/C")"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "paused inside C" "$(printf '%s' "$OUT" | jqf headline)" "Update paused inside C."
OUT=$(probe sync_abort "$W/local")
ok "abort calls C put back: it is exactly where it was, my edit and all" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])"):$(head_of "$W/local/C"):$(git -C "$W/local/C" status --porcelain)" "[('C', 'put-back')]:$C0: M other.txt"
ok "  and does not say it was left as moved" "$(printf '%s' "$OUT" | jqf detail | grep -c 'left as moved' || true)" "0"

section "A preview that upstream has moved past is refused when run, not carried out on a different plan"
mini fp
W="$SB/fp"
printf 'u1\n' > "$W/upstream/u1.txt"; git -C "$W/upstream" add u1.txt; git -C "$W/upstream" commit -qm "super: u1"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'm\n' > "$W/local/m.txt"; git -C "$W/local" add m.txt; git -C "$W/local" commit -qm "mine"
S0="$(head_of "$W/local")"
FP="$(probe sync_plan "$W/local" | "$PY" -c "import sys,json;print(json.dumps(json.load(sys.stdin)['fingerprint']))")"
printf 'u2\n' > "$W/upstream/u2.txt"; git -C "$W/upstream" add u2.txt; git -C "$W/upstream" commit -qm "super: u2"; git -C "$W/upstream" push -q origin main 2>/dev/null      # after the preview
ARGS="{\"repoPath\":\"$W/local\",\"stash\":true,\"bundles\":false,\"fingerprint\":$FP}"
OUT=$(pinvoke changes_sync_run "$ARGS")
ok "the run fetches, sees upstream moved, and refuses" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "assess-again"
ok "  nothing was touched: same commit, no backup branch, nothing paused" "$(head_of "$W/local"):$(git -C "$W/local" for-each-ref 'refs/heads/gitswitch-before-update-*' | wc -l | tr -d ' '):$([ -e "$W/local/.git/gitswitch-sync.json" ] && echo state || echo none)" "$S0:0:none"
FP="$(probe sync_plan "$W/local" | "$PY" -c "import sys,json;print(json.dumps(json.load(sys.stdin)['fingerprint']))")"
ARGS="{\"repoPath\":\"$W/local\",\"stash\":true,\"bundles\":true,\"fingerprint\":$FP}"
OUT=$(pinvoke changes_sync_run "$ARGS")
ok "previewed again, the same run goes through" "$(printf '%s' "$OUT" | jqf value.ok):$(printf '%s' "$OUT" | jqf value.headline)" "True:Updated: 1 commit of yours on top of 2 new commits"
ok "  its bundle backup is written to the update-backups folder" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,os,json;b=json.load(sys.stdin)["value"]["sync"]["bundles"];print(len(b) > 0 and all("/update-backups/" in p.replace(chr(92), "/") and os.path.isfile(p) for p in b))')" "True"

section "My commit that added a submodule is already upstream, then I bumped it: handed over, never re-written without a backup"
mini tz
W="$SB/tz"
git init -q --bare "$W/X.git"; git clone -q "$W/X.git" "$W/author-X" 2>/dev/null
printf 'x1\n' > "$W/author-X/x.txt"; git -C "$W/author-X" add -A >/dev/null; git -C "$W/author-X" commit -qm "X: one"
printf 'x2\n' >> "$W/author-X/x.txt"; git -C "$W/author-X" commit -qam "X: two"; git -C "$W/author-X" push -q origin main 2>/dev/null
X1="$(git -C "$W/author-X" rev-parse HEAD~1)"; X2="$(head_of "$W/author-X")"
# The same patch on both sides (a PR of mine that was merged by rebase), then each side moves X its own way.
for c in upstream local; do ( cd "$W/$c" && git submodule add -q "$W/X.git" X 2>/dev/null ); git -C "$W/$c/X" checkout -q "$X1"; git -C "$W/$c" add X; done
git -C "$W/upstream" commit -qm "add X (#7)"; git -C "$W/local" commit -qm "add X"
git -C "$W/upstream/X" checkout -q "$X2"; git -C "$W/upstream" add X; git -C "$W/upstream" commit -qm "move X to two"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'mine\n' > "$W/local/X/mine.txt"; git -C "$W/local/X" add mine.txt; git -C "$W/local/X" commit -qm "X: my work"; XM="$(head_of "$W/local/X")"
git -C "$W/local" add X; git -C "$W/local" commit -qm "bump X to my work"
OUT=$(probe sync_plan "$W/local")
ok "the preview says the update will pause on X" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['predicted_gitlink_conflict'])")" "none True"
OUT=$(probe sync_run "$W/local")
ok "and it does: X is handed over" "$(printf '%s' "$OUT" | jqf sync.needs_user.where_)" "submodule:X"
ok "  saying what each side records" "$(printf '%s' "$OUT" | jqf sync.needs_user.hint)" "your commit records it at $(printf '%s' "$XM" | cut -c1-7) and upstream at $(printf '%s' "$X2" | cut -c1-7)"
ok "  my commit inside X was not re-written" "$(head_of "$W/local/X")" "$XM"
ok "  and X is not called rebased" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "[('X', 'unchanged')]"
git -C "$W/local" add X       # keep where my commit put it
OUT=$(probe sync_continue "$W/local")
ok "kept as I put it: the update finishes with my bump on top" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local" log --format=%s -1):$(git -C "$W/local" rev-parse HEAD:X)" "True:bump X to my work:$XM"
ok "  and the add that upstream already had is named as left out" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was dropped because upstream already contained it: add X"

section "A commit of mine that moved a pointer back is already upstream: the submodule simply moves forward, as previewed"
mini ty A
W="$SB/ty"
for n in one two; do printf '%s\n' "$n" >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: $n"; done; git -C "$W/upstream/A" push -q origin main 2>/dev/null
ONE="$(git -C "$W/upstream/A" rev-parse HEAD~1)"; TWO="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A at two"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null; git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only "$TWO"
# The same pin on both sides, then upstream moves on.
git -C "$W/local/A" reset -q --hard "$ONE"; git -C "$W/local" add A; git -C "$W/local" commit -qm "pin A back to one"
git -C "$W/upstream/A" checkout -q "$ONE"; git -C "$W/upstream" add A; git -C "$W/upstream" commit -qm "pin A back to one (#12)"
git -C "$W/upstream/A" checkout -q main; printf 'three\n' >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: three"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
THREE="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A to three"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "the preview does not promise a pause that will not come" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['predicted_gitlink_conflict'])")" "fast-forward False"
OUT=$(probe sync_run "$W/local")
ok "the update finishes without pausing, A moved forward" "$(printf '%s' "$OUT" | jqf ok):$(head_of "$W/local/A")" "True:$THREE"
ok "  as previewed" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 0 commits of yours on top of 2 incoming."

section "A submodule git is told to ignore in diffs: my pointer commit is still mine, not called already upstream"
for world in tj tk; do
  mini $world A
  W="$SB/$world"
  for n in one two; do printf '%s\n' "$n" >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: $n"; done; git -C "$W/upstream/A" push -q origin main 2>/dev/null
  ONE="$(git -C "$W/upstream/A" rev-parse HEAD~1)"; TWO="$(head_of "$W/upstream/A")"
  if [ "$world" = tj ]; then git -C "$W/upstream" config -f .gitmodules submodule.A.ignore all; git -C "$W/upstream" add .gitmodules; fi
  git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A at two"; git -C "$W/upstream" push -q origin main 2>/dev/null
  git -C "$W/local" pull -q --ff-only 2>/dev/null; git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only "$TWO"
  [ "$world" = tk ] && git -C "$W/local" config diff.ignoreSubmodules all
  git -C "$W/local/A" reset -q --hard "$ONE"; git -C "$W/local" add A; git -C "$W/local" commit -qm "pin A back to one"
  printf 'u\n' > "$W/upstream/up.txt"; git -C "$W/upstream" add up.txt; git -C "$W/upstream" commit -qm "super: unrelated"; git -C "$W/upstream" push -q origin main 2>/dev/null
  OUT=$(probe sync_plan "$W/local")
  ok "($world) my pin is previewed as kept" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(c['subject'],c['already_upstream']) for c in d['superproject']['own_commits']])")" "[('pin A back to one', False)]"
  ok "($world)   and A is not said to move forward" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],'|',x['reason'])")" "none | your commits put its pointer at $(printf '%s' "$ONE" | cut -c1-7)"
  OUT=$(probe sync_run "$W/local")
  ok "($world) the update keeps it" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local" log --format=%s -1):$(head_of "$W/local/A")" "True:pin A back to one:$ONE"
  ok "($world)   as previewed" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 1 commit of yours on top of 1 incoming."
done

section "Files in the way are found by their real names, whatever characters they hold"
mini t1 B
W="$SB/t1"
printf 'c\n' > "$W/upstream/B/café.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: café"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
printf 'naive\n' > "$W/upstream/naïve.txt"; git -C "$W/upstream" add -A >/dev/null; git -C "$W/upstream" commit -qm "super: bump B, add naïve"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'mine\n' > "$W/local/B/café.txt"
OUT=$(probe sync_plan "$W/local")
ok "an untracked café.txt inside B blocks the update that brings one" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'])")" "blocked untracked-inside"
ok "  by its name, unquoted" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');print(' '.join(json.load(sys.stdin)['blockers']))")" "overwritten by the commits it will be moved onto: café.txt;"
rm "$W/local/B/café.txt"; printf 'mine\n' > "$W/local/naïve.txt"
OUT=$(probe sync_plan "$W/local")
ok "the same in the repository itself" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf blockers)" "False:"
ok "  by its name" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');print(' '.join(json.load(sys.stdin)['blockers']))")" "Untracked file would be overwritten by incoming commits: naïve.txt."
rm "$W/local/naïve.txt"
# An edit of mine to a file upstream changed as well: the overlap is seen under its real name too.
printf 'naive, upstream again\n' > "$W/upstream/naïve.txt"; git -C "$W/upstream" commit -qam "super: naïve again"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null; git -C "$W/local" reset -q --hard HEAD~1; git -C "$W/local" submodule update -q 2>/dev/null
printf 'naive, mine\n' > "$W/local/naïve.txt"
OUT=$(probe sync_plan "$W/local")
ok "an uncommitted edit that collides with what comes in is named too" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');print(' '.join(json.load(sys.stdin)['blockers']))")" "Your uncommitted changes to naïve.txt collide with incoming commits"

section "An untracked file where a folder has to come is in the way; a clone where a submodule comes is not"
mini tg B
W="$SB/tg"
mkdir -p "$W/upstream/B/docs"; printf 'g\n' > "$W/upstream/B/docs/guide.md"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: docs"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
mkdir -p "$W/upstream/conf"; printf 'a\n' > "$W/upstream/conf/app.yml"; git -C "$W/upstream" add -A >/dev/null; git -C "$W/upstream" commit -qm "super: bump B, add conf"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'a file, not a folder\n' > "$W/local/B/docs"
OUT=$(probe sync_plan "$W/local")
ok "a file named docs inside B, where docs/guide.md has to come, blocks" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: docs;"
rm "$W/local/B/docs"; printf 'a file, not a folder\n' > "$W/local/conf"
OUT=$(probe sync_plan "$W/local")
ok "the same in the repository itself" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf blockers)" "False:"
ok "  naming the file" "$(printf '%s' "$OUT" | jqf blockers)" "conf"
rm "$W/local/conf"
# A folder of that name with other files in it is no obstacle: the new file arrives beside them.
mkdir -p "$W/local/conf" "$W/local/B/docs"; printf 'mine\n' > "$W/local/conf/other.yml"; printf 'mine\n' > "$W/local/B/docs/other.md"
OUT=$(probe sync_plan "$W/local")
ok "untracked neighbours in the same folder are not in the way" "$(printf '%s' "$OUT" | jqf can_run)" "True"
OUT=$(probe sync_run "$W/local")
ok "  and the update goes through beside them" "$(printf '%s' "$OUT" | jqf ok):$(cat "$W/local/conf/other.yml"):$(cat "$W/local/B/docs/other.md"):$(cat "$W/local/conf/app.yml")" "True:mine:mine:a"
# Upstream adds a submodule where I already have a clone of it by hand.
git init -q --bare "$W/sdk.git"; git clone -q "$W/sdk.git" "$W/author-sdk" 2>/dev/null
printf 's\n' > "$W/author-sdk/s.txt"; git -C "$W/author-sdk" add -A >/dev/null; git -C "$W/author-sdk" commit -qm "sdk: one"; git -C "$W/author-sdk" push -q origin main 2>/dev/null
( cd "$W/upstream" && git submodule add -q "$W/sdk.git" sdk 2>/dev/null ); git -C "$W/upstream" commit -qm "super: add sdk"; git -C "$W/upstream" push -q origin main 2>/dev/null
git clone -q "$W/sdk.git" "$W/local/sdk" 2>/dev/null; printf 'kept\n' > "$W/local/sdk/notes.txt"
OUT=$(probe sync_plan "$W/local")
ok "a clone of mine where upstream adds a submodule does not block" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf blockers)" "True:[]"
OUT=$(probe sync_run "$W/local")
ok "  the update takes it as that submodule's checkout, my file in it untouched" "$(git -C "$W/local" rev-parse HEAD:sdk):$(cat "$W/local/sdk/notes.txt")" "$(head_of "$W/author-sdk"):kept"
ok "  and the checks pass: a clone that became a submodule is not an untracked file that vanished" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf sync.verified.untracked_unchanged)" "True:True"

section "A folder my commits turned into a submodule is one I added, not a pointer upstream is said to record"
mini tf
W="$SB/tf"
mkdir -p "$W/upstream/lib"; printf 'vendored\n' > "$W/upstream/lib/v.txt"; git -C "$W/upstream" add -A >/dev/null; git -C "$W/upstream" commit -qm "super: vendored lib"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null
git init -q --bare "$W/L.git"; git clone -q "$W/L.git" "$W/author-L" 2>/dev/null
printf 'l\n' > "$W/author-L/l.txt"; git -C "$W/author-L" add -A >/dev/null; git -C "$W/author-L" commit -qm "L: one"; git -C "$W/author-L" push -q origin main 2>/dev/null
cp -R "$W/local" "$W/local2"
git -C "$W/local" rm -rq lib; git -C "$W/local" commit -qm "drop vendored lib"
( cd "$W/local" && git submodule add -q "$W/L.git" lib 2>/dev/null ); git -C "$W/local" commit -qm "lib as a submodule"
printf 'u\n' > "$W/upstream/up.txt"; git -C "$W/upstream" add up.txt; git -C "$W/upstream" commit -qm "super: unrelated"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "the preview calls it mine, and can run" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],'|',x['reason'])")" "True:none | added by your commits; upstream does not have it"
OUT=$(probe sync_run "$W/local")
ok "  and the update keeps both commits on top" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD)" "True:lib as a submodule|drop vendored lib|"
# Staged and not committed yet, on a copy: said as what it is.
git -C "$W/local2" rm -rq lib; ( cd "$W/local2" && git submodule add -q "$W/L.git" lib 2>/dev/null )
OUT=$(probe sync_plan "$W/local2")
ok "staged and not committed: the reason is that, not a folder's id shown as a pointer" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['remedy'],'|',x['reason'])")" "commit-first | it was added here but is not committed yet"

section "A lost commit is not excused by a namesake that was replayed onto changed surroundings"
for world in lw lx; do
  mini $world
  W="$SB/$world"; seed_file $world
  printf 'readme (upstream)\n' > "$W/upstream/README.md"; edit_line "$W/upstream/F.txt" 4 "(upstream)"
  git -C "$W/upstream" commit -qam "super: readme and line 4"; git -C "$W/upstream" push -q origin main 2>/dev/null
  # Two commits with one subject. In lw they are written a day apart, in lx in the same second.
  D1="2026-01-05T10:00:00"; D2="2026-01-06T10:00:00"; [ "$world" = lx ] && D2="$D1"
  printf 'readme (mine)\n' > "$W/local/README.md"; GIT_AUTHOR_DATE="$D1" git -C "$W/local" commit -qam "wip"
  edit_line "$W/local/F.txt" 2 "(mine)"; GIT_AUTHOR_DATE="$D2" git -C "$W/local" commit -qam "wip"
  probe sync_plan "$W/local" >/dev/null
  OUT=$(probe sync_run "$W/local")
  ok "($world) paused on the README" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "README.md"
  git -C "$W/local" rebase --skip >/dev/null 2>&1      # in a terminal: the README commit is thrown away
  OUT=$(probe sync_continue "$W/local")
  ok "($world) one wip is on top, the other is gone: a failed check" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf sync.verified.own_on_top):$(subjects "$W/local" origin/main..HEAD):$(sed -n 2p "$W/local/F.txt")" "False:False:wip|:line 2 (mine)"
  ok "($world)   named as missing, not as already upstream" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours is missing after the replay and not found upstream: wip."
  ok "($world)   and nothing is called dropped-because-upstream-had-it" "$(printf '%s' "$OUT" | jqf detail | grep -c 'already contained' || true)" "0"
done

section "Resolved into nothing is measured when the update continues, so a namesake keeps its own copy"
mini rn2
W="$SB/rn2"; seed_file rn2
printf 'readme (upstream)\n' > "$W/upstream/README.md"; edit_line "$W/upstream/F.txt" 4 "(upstream)"
git -C "$W/upstream" commit -qam "super: readme and line 4"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'readme (mine)\n' > "$W/local/README.md"; git -C "$W/local" commit -qam "wip"; GONE="$(head_of "$W/local")"
edit_line "$W/local/F.txt" 2 "(mine)"; git -C "$W/local" commit -qam "wip"
probe sync_plan "$W/local" >/dev/null
probe sync_run "$W/local" >/dev/null
probe tree_resolve_side "$W/local" "theirs,README.md" >/dev/null        # upstream's README: my first wip has nothing left
OUT=$(probe sync_continue "$W/local")
ok "the update finishes with the other wip on top" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD):$(sed -n 2p "$W/local/F.txt")" "True:wip|:line 2 (mine)"
ok "  one left out, said as resolved into nothing" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was left out: nothing of it remained once its conflict was resolved — wip."
ok "  and marked so for the card" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(c['subject'],c['resolved_away'],c['already_upstream']) for c in d['sync']['own_dropped']])")" "[('wip', True, False)]"
ok "  the one named is the README commit, not its namesake" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([c['oid'] for c in d['sync']['own_dropped']])")" "['$GONE']"

section "A commit with no subject at all is replayed and counted like any other"
mini es
W="$SB/es"; seed_file es
edit_line "$W/upstream/F.txt" 4 "(upstream)"; edit_line "$W/upstream/F.txt" 9 "(upstream)"
git -C "$W/upstream" commit -qam "super: lines 4 and 9"; git -C "$W/upstream" push -q origin main 2>/dev/null
# Each of mine lands next to a line upstream changed: replayed cleanly, but not as the same patch.
edit_line "$W/local/F.txt" 2 "(mine)"; git -C "$W/local" commit -q --allow-empty-message -am ""
printf 'b\n' > "$W/local/b.txt"; git -C "$W/local" add b.txt; git -C "$W/local" commit -qm "add b"
edit_line "$W/local/F.txt" 11 "(mine)"; git -C "$W/local" commit -q --allow-empty-message -am ""
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update finishes: three of mine on top" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf headline)" "True:Updated: 3 commits of yours on top of 1 new commit"
ok "  none called dropped" "$(printf '%s' "$OUT" | jqf sync.own_dropped)" "[]"
ok "  as previewed" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 3 commits of yours on top of 1 incoming."

section "Abort puts back a handed-over submodule I had moved as the hand-over said"
mini hb A
W="$SB/hb"
for n in one two; do printf '%s\n' "$n" >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: $n"; done; git -C "$W/upstream/A" push -q origin main 2>/dev/null
ONE="$(git -C "$W/upstream/A" rev-parse HEAD~1)"; TWO="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A at two"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null; git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only "$TWO"
printf 'three\n' >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: three"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
THREE="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A to three"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local/A" reset -q --hard "$ONE"; git -C "$W/local" add A; git -C "$W/local" commit -qm "pin A back to one"
S0="$(head_of "$W/local")"
cp -R "$W/local" "$W/local2"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update hands A over" "$(printf '%s' "$OUT" | jqf sync.needs_user.where_)" "submodule:A"
git -C "$W/local/A" merge -q --ff-only "$THREE"       # "move it to upstream's" — then I change my mind
OUT=$(probe sync_abort "$W/local")
ok "abort puts A back where it stood, and says so" "$(printf '%s' "$OUT" | jqf headline):$(printf '%s' "$OUT" | jqf detail)" "Update aborted.:A: put back at $(printf '%s' "$ONE" | cut -c1-7)"
ok "  A and the branch are where they were" "$(head_of "$W/local/A"):$(on_branch "$W/local/A"):$(head_of "$W/local")" "$ONE:main:$S0"
ok "  nothing is off its recorded commit" "$(git -C "$W/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"
ok "  the outcome calls A put back" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "[('A', 'put-back')]"
# On a copy: I put A on a new commit of my own instead. Abort must not move a branch off work nothing else holds.
probe sync_plan "$W/local2" >/dev/null; probe sync_run "$W/local2" >/dev/null
printf 'new work\n' >> "$W/local2/A/A.txt"; git -C "$W/local2/A" commit -qam "A: new work of mine"; MINE="$(head_of "$W/local2/A")"
OUT=$(probe sync_abort "$W/local2")
ok "a commit of mine made there meanwhile is not walked off: A is left, and named" "$(printf '%s' "$OUT" | jqf detail):$(head_of "$W/local2/A")" "A: left at $(printf '%s' "$MINE" | cut -c1-7), where you put it; $(printf '%s' "$MINE" | cut -c1-7) holds commits nothing else has, and it stood at $(printf '%s' "$ONE" | cut -c1-7) before the update:$MINE"
ok "  the outcome says left as moved" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "[('A', 'left-as-moved')]"

section "The branch reset by hand while a submodule's rebase was paused: Continue sees an abort and puts back the rest"
mini ar B C
W="$SB/ar"
printf 'b2\n' > "$W/upstream/B/b2.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: upstream 1"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
printf 'C v1, rewritten\nline2\nline3\n' > "$W/upstream/C/C.txt"; git -C "$W/upstream/C" commit -q --amend -am "C: initial, rewritten"; git -C "$W/upstream/C" push -q -f origin main 2>/dev/null
git -C "$W/upstream" add B C >/dev/null; git -C "$W/upstream" commit -qm "super: bump B, rewrite C"; git -C "$W/upstream" push -q origin main 2>/dev/null
printf 'mine\n' > "$W/local/mine.txt"; git -C "$W/local" add mine.txt; git -C "$W/local" commit -qm "my own commit"
B0="$(head_of "$W/local/B")"; C0="$(head_of "$W/local/C")"; S0="$(head_of "$W/local")"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update pauses inside C, the branch already replayed" "$(printf '%s' "$OUT" | jqf headline):$([ "$(head_of "$W/local")" != "$S0" ] && echo replayed || echo still)" "Update paused inside C.:replayed"
BK="$(git -C "$W/local" branch --list 'gitswitch-before-update-*' --format='%(refname:short)' | head -1)"
git -C "$W/local/C" rebase --abort >/dev/null 2>&1; git -C "$W/local" reset -q --hard "$BK"      # in a terminal
OUT=$(probe sync_continue "$W/local")
ok "Continue update sees it was undone by hand" "$(printf '%s' "$OUT" | jqf headline)" "Update aborted."
ok "  and puts B back too, saying so" "$(printf '%s' "$OUT" | jqf detail)" "B: put back at $(printf '%s' "$B0" | cut -c1-7)"
ok "  B, C and the branch are where they were" "$(head_of "$W/local/B"):$(head_of "$W/local/C"):$(head_of "$W/local")" "$B0:$C0:$S0"
ok "  nothing is off its recorded commit" "$(git -C "$W/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"

section "My pointer already contains upstream's, but git cannot see that from the base: recorded as mine, no pause, nothing re-written"
mini cu A
W="$SB/cu"
# Upstream re-writes A's history; I rebuild my work on top of the new line and record it.
printf 'A v1, rewritten\nline2\nline3\n' > "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -q --amend -am "A: initial, rewritten"; git -C "$W/upstream/A" push -q -f origin main 2>/dev/null
NEWA="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A rewritten"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" reset -q --hard "$NEWA"
printf 'mine\n' > "$W/local/A/mine.txt"; git -C "$W/local/A" add mine.txt; git -C "$W/local/A" commit -qm "A: my work on the new line"; MINE="$(head_of "$W/local/A")"
git -C "$W/local" add A; git -C "$W/local" commit -qm "bump A to my work"
OUT=$(probe sync_plan "$W/local")
ok "the preview says my pointer is kept" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],'|',x['reason'])")" "none | already contains $(printf '%s' "$NEWA" | cut -c1-7); your commit's pointer will be kept"
OUT=$(probe sync_run "$W/local")
ok "the update finishes without a pause" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf sync.phase)" "True:done"
ok "  recording my commit, which A still sits on, un-rewritten" "$(git -C "$W/local" rev-parse HEAD:A):$(head_of "$W/local/A")" "$MINE:$MINE"
ok "  as previewed" "$(printf '%s' "$OUT" | jqf detail)" "As previewed: 1 commit of yours on top of 1 incoming."

section "A commit whose message starts with '#' keeps its message when its conflict is resolved and the update continues"
mini hs
W="$SB/hs"; seed_file hs
edit_line "$W/upstream/F.txt" 3 "(upstream)"; git -C "$W/upstream" commit -qam "super: line 3"; git -C "$W/upstream" push -q origin main 2>/dev/null
edit_line "$W/local/F.txt" 3 "(mine)"; printf '#123 fix line 3\n\nBody of the fix.\n# a body line that looks like a comment\n' > "$W/msg"; git -C "$W/local" commit -q -a -F "$W/msg" --cleanup=verbatim
edit_line "$W/local/F.txt" 9 "(mine)"; git -C "$W/local" commit -qam "mine: line 9"
cp -R "$W/local" "$W/local2"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update pauses on F.txt" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "F.txt"
probe tree_resolve_side "$W/local" "mine,F.txt" >/dev/null
OUT=$(probe sync_continue "$W/local")
ok "Continue finishes with both commits on top, as previewed" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf headline):$(printf '%s' "$OUT" | jqf detail)" "True:Updated: 2 commits of yours on top of 1 new commit from origin/main.:As previewed: 2 commits of yours on top of 1 incoming."
ok "  the message is kept letter for letter, '#' lines and all" "$(git -C "$W/local" log -1 --format=%B HEAD~1 | tr '\n' '|')" "#123 fix line 3||Body of the fix.|# a body line that looks like a comment|"
ok "  and nothing is called dropped" "$(printf '%s' "$OUT" | jqf sync.own_dropped)" "[]"
# The same with nothing but a '#' line: left to git, that is an empty message and a dead end.
printf '#123 only a subject\n' > "$W/msg2"
git -C "$W/local2" reset -q --hard HEAD~2; edit_line "$W/local2/F.txt" 3 "(mine)"; git -C "$W/local2" commit -q -a -F "$W/msg2" --cleanup=verbatim
edit_line "$W/local2/F.txt" 9 "(mine)"; git -C "$W/local2" commit -qam "mine: line 9"
probe sync_plan "$W/local2" >/dev/null; probe sync_run "$W/local2" >/dev/null
probe tree_resolve_side "$W/local2" "mine,F.txt" >/dev/null
OUT=$(probe sync_continue "$W/local2")
ok "a message that is only a '#' line is kept too, and the update finishes" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local2" log --format=%s origin/main..HEAD | tr '\n' '|')" "True:mine: line 9|#123 only a subject|"
ok "  with the author and time of the original" "$(git -C "$W/local2" log -1 --format='%an %aI' HEAD~1)" "$(git -C "$W/local2" log -1 --format='%an %aI' "$(git -C "$W/local2" branch --list 'gitswitch-before-update-*' --format='%(refname:short)' | head -1)~1")"

section "Abort never walks a submodule's branch off a commit nothing else holds"
mini st A
W="$SB/st"
for n in one two; do printf '%s\n' "$n" >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: $n"; done; git -C "$W/upstream/A" push -q origin main 2>/dev/null
ONE="$(git -C "$W/upstream/A" rev-parse HEAD~1)"; TWO="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A at two"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null; git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only "$TWO"
printf 'three\n' >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: three"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
THREE="$(head_of "$W/upstream/A")"
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: A to three"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local/A" reset -q --hard "$ONE"; git -C "$W/local" add A; git -C "$W/local" commit -qm "pin A back to one"
probe sync_plan "$W/local" >/dev/null; probe sync_run "$W/local" >/dev/null
# In a terminal: a commit on A's branch, then the checkout moved to upstream's (published) commit.
printf 'x\n' >> "$W/local/A/A.txt"; git -C "$W/local/A" commit -qam "A: X, mine alone"; X="$(head_of "$W/local/A")"
git -C "$W/local/A" checkout -q "$THREE"; git -C "$W/local" add A
OUT=$(probe sync_abort "$W/local")
ok "the checkout sits on a published commit, but the branch holds X: A is left, saying why" "$(printf '%s' "$OUT" | jqf detail)" "A: left at $(printf '%s' "$THREE" | cut -c1-7), where you put it; $(printf '%s' "$X" | cut -c1-7) holds commits nothing else has, and it stood at $(printf '%s' "$ONE" | cut -c1-7) before the update"
ok "  the branch still holds X" "$(git -C "$W/local/A" rev-parse main):$(git -C "$W/local/A" branch -a --contains "$X" | tr -d ' *\n')" "$X:main"
ok "  and the outcome says left as moved" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "[('A', 'left-as-moved')]"

section "Abort leaves a submodule the update rebased when a commit was added to it since"
mini sx B C
W="$SB/sx"
printf 'b2\n' > "$W/upstream/B/b2.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: upstream 1"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
printf 'C v1, rewritten\nline2\nline3\n' > "$W/upstream/C/C.txt"; git -C "$W/upstream/C" commit -q --amend -am "C: initial, rewritten"; git -C "$W/upstream/C" push -q -f origin main 2>/dev/null
git -C "$W/upstream" add B C >/dev/null; git -C "$W/upstream" commit -qm "super: bump B, rewrite C"; git -C "$W/upstream" push -q origin main 2>/dev/null
# My own work in B, so B is rebased at its pointer stop; C pauses afterwards.
printf 'mine\n' > "$W/local/B/mine.txt"; git -C "$W/local/B" add mine.txt; git -C "$W/local/B" commit -qm "B: mine"; git -C "$W/local" add B; git -C "$W/local" commit -qm "seal B"
B0="$(head_of "$W/local/B")"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update pauses inside C, with B rebased" "$(printf '%s' "$OUT" | jqf headline):$([ "$(head_of "$W/local/B")" != "$B0" ] && echo rebased || echo not)" "Update paused inside C.:rebased"
printf 'more\n' > "$W/local/B/more.txt"; git -C "$W/local/B" add more.txt; git -C "$W/local/B" commit -qm "B: more, during the pause"; BX="$(head_of "$W/local/B")"
OUT=$(probe sync_abort "$W/local")
ok "abort leaves B where it is and says what it holds" "$(printf '%s' "$OUT" | jqf detail)" "B: left as moved: $(printf '%s' "$BX" | cut -c1-7) holds commits made since the update, which nothing else has; it stood at $(printf '%s' "$B0" | cut -c1-7) before the update"
ok "  B still holds that commit" "$(head_of "$W/local/B")" "$BX"
ok "  C is back where it was" "$(git -C "$W/local/C" status --porcelain | grep -c '^UU' || true):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "0:[('B', 'left-as-moved'), ('C', 'put-back')]"

section "A rename of mine is not foreseen as dropped because upstream has the new name"
mini fr
W="$SB/fr"
for i in 1 2 3 4 5 6 7 8 9 10; do printf 'line %s\n' "$i"; done > "$W/upstream/old.txt"; git -C "$W/upstream" add old.txt; git -C "$W/upstream" commit -qm "super: old.txt"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null
cp "$W/upstream/old.txt" "$W/upstream/new.txt"; git -C "$W/upstream" add new.txt; git -C "$W/upstream" commit -qm "super: copy to new.txt"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" mv old.txt new.txt; git -C "$W/local" commit -qm "rename old.txt to new.txt"
OUT=$(probe sync_plan "$W/local")
ok "the preview keeps my rename" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(c['subject'],c['already_upstream']) for c in d['superproject']['own_commits']])")" "[('rename old.txt to new.txt', False)]"
OUT=$(probe sync_run "$W/local")
ok "the update keeps it too, and says so once" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf detail):$(printf '%s' "$OUT" | jqf sync.own_dropped)" "True:As previewed: 1 commit of yours on top of 1 incoming.:[]"
ok "  old.txt is gone, as my commit said" "$([ -e "$W/local/old.txt" ] && echo present || echo gone)" "gone"
# A file whose name is pathspec magic to git: compared as a name, not read as magic.
mini fc
W="$SB/fc"
printf 'note 1\n' > "$W/upstream/:notes"; git -C "$W/upstream" add -- ":(literal):notes"; git -C "$W/upstream" commit -qm "super: notes"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null
printf 'my note\n' >> "$W/local/:notes"; git -C "$W/local" commit -qam "edit my notes"
printf 'u\n' > "$W/upstream/up.txt"; git -C "$W/upstream" add up.txt; git -C "$W/upstream" commit -qm "super: unrelated"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "a commit to a file called :notes is previewed as kept" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(c['subject'],c['already_upstream']) for c in d['superproject']['own_commits']])")" "[('edit my notes', False)]"
OUT=$(probe sync_run "$W/local")
ok "  and kept" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf detail)" "True:As previewed: 1 commit of yours on top of 1 incoming."

section "A clone of mine where upstream adds a submodule is adopted only when it sits on the recorded commit"
mini ad
W="$SB/ad"
git init -q --bare "$W/sdk.git"; git clone -q "$W/sdk.git" "$W/author-sdk" 2>/dev/null
printf 's\n' > "$W/author-sdk/s.txt"; git -C "$W/author-sdk" add -A >/dev/null; git -C "$W/author-sdk" commit -qm "sdk: one"; git -C "$W/author-sdk" push -q origin main 2>/dev/null
S1="$(head_of "$W/author-sdk")"
printf 's2\n' >> "$W/author-sdk/s.txt"; git -C "$W/author-sdk" commit -qam "sdk: two"; git -C "$W/author-sdk" push -q origin main 2>/dev/null
( cd "$W/upstream" && git submodule add -q "$W/sdk.git" sdk 2>/dev/null ); git -C "$W/upstream" commit -qm "super: add sdk"; git -C "$W/upstream" push -q origin main 2>/dev/null
cp -R "$W/local" "$W/local2"; cp -R "$W/local" "$W/local3"
git clone -q "$W/sdk.git" "$W/local/sdk" 2>/dev/null; git -C "$W/local/sdk" reset -q --hard "$S1"
OUT=$(probe sync_plan "$W/local")
ok "a clone at another commit is in the way, and said so" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf blockers)" "False:"
ok "  by name" "$(printf '%s' "$OUT" | jqf blockers)" "Untracked file would be overwritten by incoming commits: sdk/."
git init -q "$W/local2/sdk"; printf 'unrelated\n' > "$W/local2/sdk/u.txt"
OUT=$(probe sync_plan "$W/local2")
ok "an unrelated repository there is in the way too" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf blockers)" "False:"
git clone -q "$W/sdk.git" "$W/local3/sdk" 2>/dev/null
OUT=$(probe sync_plan "$W/local3")
ok "a clone on the recorded commit is not" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf blockers)" "True:[]"
OUT=$(probe sync_run "$W/local3")
ok "  and the update adopts it, every check passing" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf sync.verified.untracked_unchanged):$(git -C "$W/local3" rev-parse HEAD:sdk):$(head_of "$W/local3/sdk")" "True:True:$(head_of "$W/author-sdk"):$(head_of "$W/author-sdk")"

section "A pointer commit of mine that is already upstream keeps nothing when upstream then moved the pointer back"
mini tb A
W="$SB/tb"
for n in one two; do printf '%s\n' "$n" >> "$W/upstream/A/A.txt"; git -C "$W/upstream/A" commit -qam "A: $n"; done; git -C "$W/upstream/A" push -q origin main 2>/dev/null
ONE="$(git -C "$W/upstream/A" rev-parse HEAD~1)"; TWO="$(head_of "$W/upstream/A")"
git -C "$W/local/A" fetch -q origin 2>/dev/null; git -C "$W/local/A" merge -q --ff-only "$TWO"; git -C "$W/local" add A; git -C "$W/local" commit -qm "bump A to two"
git -C "$W/upstream/A" checkout -q "$TWO"; git -C "$W/upstream" add A; git -C "$W/upstream" commit -qm "bump A to two (#12)"        # the same patch, merged by rebase
git -C "$W/upstream/A" checkout -q "$ONE"; git -C "$W/upstream" add A; git -C "$W/upstream" commit -qm "back A off to one"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "my bump is foreseen as already upstream" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(c['subject'],c['already_upstream']) for c in d['superproject']['own_commits']])")" "[('bump A to two', True)]"
ok "  and A is not promised to be kept: the checkout will sit ahead of what the branch records" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],'|',x['reason'])")" "ahead | your commits sit on top of $(printf '%s' "$ONE" | cut -c1-7) already; the superproject does not record them yet"
OUT=$(probe sync_run "$W/local")
ok "the run does what the preview said" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local" rev-parse HEAD:A):$(head_of "$W/local/A"):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "True:$ONE:$TWO:[('A', 'left-ahead')]"

section "Files in the way are compared the way the disk compares them"
mini ic B
W="$SB/ic"
mkdir -p "$W/upstream/B/docs"; printf 'g\n' > "$W/upstream/B/docs/guide.md"; printf 'r\n' > "$W/upstream/B/readme.md"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: docs"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
git -C "$W/upstream" add B >/dev/null; git -C "$W/upstream" commit -qm "super: bump B"; git -C "$W/upstream" push -q origin main 2>/dev/null
if [ "$(git -C "$W/local/B" config --type=bool core.ignorecase)" = "true" ]; then
  printf 'a file\n' > "$W/local/B/Docs"
  OUT=$(probe sync_plan "$W/local")
  ok "on a case-insensitive disk, a file called Docs is in the way of docs/guide.md" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: Docs;"
  rm "$W/local/B/Docs"; printf 'mine\n' > "$W/local/B/README.MD"
  OUT=$(probe sync_plan "$W/local")
  ok "  and README.MD of readme.md" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: README.MD;"
  rm "$W/local/B/README.MD"
else
  printf '  (case-sensitive disk here: the Docs/docs checks need a case-insensitive one)\n'
fi
# An untracked repository sitting where a folder of incoming files goes: in the way when it already holds one of them, not otherwise.
git init -q "$W/local/B/docs"; printf 'mine\n' > "$W/local/B/docs/other.md"
OUT=$(probe sync_plan "$W/local")
ok "an untracked repository at docs/ without guide.md is not in the way" "$(printf '%s' "$OUT" | jqf can_run)" "True"
printf 'mine\n' > "$W/local/B/docs/guide.md"
OUT=$(probe sync_plan "$W/local")
ok "  holding guide.md, it is — named by the file git would refuse to overwrite" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: docs/guide.md;"

section "Files in the way are compared the way the disk compares them (every alphabet)"
mini ic2 B
W="$SB/ic2"
mkdir -p "$W/upstream/B/émile"; printf 'n\n' > "$W/upstream/B/émile/n.txt"; printf 'a\n' > "$W/upstream/B/Ärger.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: accents"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
mkdir -p "$W/upstream/émile"; printf 'n\n' > "$W/upstream/émile/n.txt"; git -C "$W/upstream" add -A >/dev/null; git -C "$W/upstream" commit -qm "super: bump B, add émile"; git -C "$W/upstream" push -q origin main 2>/dev/null
if [ "$(git -C "$W/local/B" config --type=bool core.ignorecase)" = "true" ]; then
  printf 'a file\n' > "$W/local/B/ÉMILE"
  OUT=$(probe sync_plan "$W/local")
  ok "on a case-insensitive disk, a file called ÉMILE is in the way of émile/n.txt" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: ÉMILE;"
  rm "$W/local/B/ÉMILE"; printf 'mine\n' > "$W/local/B/ärger.txt"
  OUT=$(probe sync_plan "$W/local")
  ok "  and ärger.txt of Ärger.txt" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: ärger.txt;"
  rm "$W/local/B/ärger.txt"
  mkdir -p "$W/local/ÉMILE"; printf 'mine\n' > "$W/local/ÉMILE/N.TXT"
  OUT=$(probe sync_plan "$W/local")
  ok "  in the repository itself, ÉMILE/N.TXT is in the way of émile/n.txt" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');print(' '.join(json.load(sys.stdin)['blockers']))")" "Untracked file would be overwritten by incoming commits: ÉMILE/N.TXT."
  rm -r "$W/local/ÉMILE"
  # The disk decides, not a setting: with core.ignorecase unset the disk still folds.
  git -C "$W/local/B" config --unset core.ignorecase; printf 'mine\n' > "$W/local/B/ÄRGER.TXT"
  OUT=$(probe sync_plan "$W/local")
  ok "  with core.ignorecase unset the answer is the disk's, still" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: ÄRGER.TXT;"
  rm "$W/local/B/ÄRGER.TXT"; git -C "$W/local/B" config core.ignorecase true
else
  printf '  (case-sensitive disk here: the folded-name checks need a case-insensitive one)\n'
fi
# An ignored file at an incoming path is not in the way: git overwrites it.
EXCL="$(git -C "$W/local/B" rev-parse --path-format=absolute --git-path info/exclude)"; mkdir -p "$(dirname "$EXCL")"
printf 'ignored\n' > "$W/local/B/Ärger.txt"; printf 'Ärger.txt\n' > "$EXCL"
OUT=$(probe sync_plan "$W/local")
ok "an ignored file at an incoming path is not in the way" "$(printf '%s' "$OUT" | jqf can_run)" "True"
rm "$W/local/B/Ärger.txt"; : > "$EXCL"
# An empty untracked folder where a file comes is not in the way either; one with a file in it is.
mkdir -p "$W/local/B/Ärger.txt"
OUT=$(probe sync_plan "$W/local")
ok "an empty untracked folder where a file comes is not in the way" "$(printf '%s' "$OUT" | jqf can_run)" "True"
printf 'x\n' > "$W/local/B/Ärger.txt/x"
OUT=$(probe sync_plan "$W/local")
ok "  one with a file in it is, named as a folder" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['remedy'],'|',x['reason'])")" "blocked untracked-inside | untracked file inside it would be overwritten by the commits it will be moved onto: Ärger.txt/;"
rm -r "$W/local/B/Ärger.txt"
# Many files in the way: the sentence names ten and counts the rest.
mkdir -p "$W/upstream/B/gen"; for i in $(seq 1 30); do printf '%s\n' "$i" > "$W/upstream/B/gen/f$i.txt"; done; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: gen"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
git -C "$W/upstream" add B >/dev/null; git -C "$W/upstream" commit -qm "super: bump B"; git -C "$W/upstream" push -q origin main 2>/dev/null
mkdir -p "$W/local/B/gen"; for i in $(seq 1 30); do printf 'mine\n' > "$W/local/B/gen/f$i.txt"; done
OUT=$(probe sync_plan "$W/local")
ok "thirty files in the way: ten are named and the rest counted" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];r=x['reason'];print(x['remedy'], r.count('gen/f'), 'and 20 more' in r)")" "untracked-inside 10 True"
for i in $(seq 31 60); do printf 'mine\n' > "$W/local/B/gen/f$i.txt"; done
for i in $(seq 31 60); do printf '%s\n' "$i" > "$W/upstream/B/gen/f$i.txt"; done; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: more gen"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
git -C "$W/upstream" add B >/dev/null; git -C "$W/upstream" commit -qm "super: bump B again"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "sixty in the way: the count stops looking at fifty and says so" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];r=x['reason'];print(x['remedy'], r.count('gen/f'), 'and at least 40 more' in r)")" "untracked-inside 10 True"
rm -r "$W/local/B/gen"
# Thousands of ignored files on disk that upstream starts tracking: not in the way, and not slow to say so.
mini ig
W="$SB/ig"
mkdir -p "$W/upstream/gen"; "$PY" -c "
import sys
for i in range(2000):
    open('%s/gen/f%d.txt' % (sys.argv[1], i), 'w').write('%d\n' % i)
" "$W/upstream"; git -C "$W/upstream" add -A >/dev/null; git -C "$W/upstream" commit -qm "super: track gen"; git -C "$W/upstream" push -q origin main 2>/dev/null
mkdir -p "$W/local/gen"; "$PY" -c "
import sys
for i in range(2000):
    open('%s/gen/f%d.txt' % (sys.argv[1], i), 'w').write('mine\n')
" "$W/local"; printf 'gen/\n' > "$W/local/.git/info/exclude"
T0=$(date +%s); OUT=$(probe sync_plan "$W/local"); T1=$(date +%s)
ok "two thousand ignored files where upstream's come are not in the way (git overwrites ignored files)" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | jqf blockers)" "True:[]"
ok "  and the preview took seconds, not minutes" "$([ $((T1-T0)) -lt 25 ] && echo quick || echo "slow: $((T1-T0))s")" "quick"
# On macOS a name may be spelled decomposed on disk and composed in the tree; with core.precomposeunicode off, git sees two names — the disk sees one.
if [ "$(uname)" = Darwin ]; then
  mini nf
  W="$SB/nf"
  printf 'c\n' > "$W/upstream/café.txt"; git -C "$W/upstream" add -A >/dev/null; git -C "$W/upstream" commit -qm "super: café"; git -C "$W/upstream" push -q origin main 2>/dev/null
  git -C "$W/local" config core.precomposeunicode false
  printf 'mine\n' > "$W/local/$(printf 'cafe\314\201.txt')"
  OUT=$(probe sync_plan "$W/local")
  ok "a decomposed café.txt on disk is in the way of the composed one, whatever git's normalisation setting" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;sys.stdout.reconfigure(encoding='utf-8');print(' '.join(json.load(sys.stdin)['blockers']))")" "False:Untracked file would be overwritten by incoming commits: cafe"
fi

section "A stop committed by the app before git refused the continue is not committed twice, nor called left out"
mini uc
W="$SB/uc"; seed_file uc
printf 'notes\n' > "$W/upstream/notes.txt"; git -C "$W/upstream" add notes.txt; git -C "$W/upstream" commit -qm "super: notes"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/local" pull -q --ff-only 2>/dev/null
edit_line "$W/upstream/F.txt" 3 "(upstream)"; git -C "$W/upstream" commit -qam "super: line 3"; git -C "$W/upstream" push -q origin main 2>/dev/null
edit_line "$W/local/F.txt" 3 "(mine)"; git -C "$W/local" commit -qam "#7 fix line 3"
edit_line "$W/local/F.txt" 9 "(mine)"; git -C "$W/local" commit -qam "mine: line 9"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update pauses on F.txt" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "F.txt"
probe tree_resolve_side "$W/local" "mine,F.txt" >/dev/null
printf 'edited during the pause\n' >> "$W/local/notes.txt"          # an unstaged edit to another tracked file
OUT=$(probe sync_continue "$W/local")
ok "Continue is refused by git for the unstaged edit, and nothing was committed" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local" log --format=%s -1):$([ -e "$(git -C "$W/local" rev-parse --path-format=absolute --git-path MERGE_MSG)" ] && echo open || echo closed)" "False:super: line 3:open"
git -C "$W/local" checkout -q -- notes.txt
OUT=$(probe sync_continue "$W/local")
ok "discarded, the next Continue finishes with both commits on top, once each" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD)" "True:mine: line 9|#7 fix line 3|"
ok "  as previewed, nothing left out" "$(printf '%s' "$OUT" | jqf detail):$(printf '%s' "$OUT" | jqf sync.own_dropped)" "As previewed: 2 commits of yours on top of 1 incoming.:[]"
# The other way round, on a copy: the person commits the stop in a terminal, then presses Continue.
mini uc2
W="$SB/uc2"; seed_file uc2
edit_line "$W/upstream/F.txt" 3 "(upstream)"; git -C "$W/upstream" commit -qam "super: line 3"; git -C "$W/upstream" push -q origin main 2>/dev/null
edit_line "$W/local/F.txt" 3 "(mine)"; git -C "$W/local" commit -qam "fix line 3"
edit_line "$W/local/F.txt" 9 "(mine)"; git -C "$W/local" commit -qam "mine: line 9"
probe sync_plan "$W/local" >/dev/null; probe sync_run "$W/local" >/dev/null
probe tree_resolve_side "$W/local" "mine,F.txt" >/dev/null
GIT_EDITOR=true git -C "$W/local" commit -q --no-verify -C "$(cat "$(git -C "$W/local" rev-parse --path-format=absolute --git-path rebase-merge/stopped-sha)")"     # in a terminal
OUT=$(probe sync_continue "$W/local")
ok "a stop committed by hand is carried on, not called resolved into nothing" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD):$(printf '%s' "$OUT" | jqf sync.own_dropped)" "True:mine: line 9|fix line 3|:[]"

# Several pointer commits of mine, with upstream moving the pointer too: the
# submodule is rebased once, and each commit records the rewritten copy of the
# commit it pointed at. Fixture: A at a0; upstream adds u1 to A and bumps.
multi_world() {  # multi_world <name> <n own A commits, each sealed by a pointer commit>
  mini "$1" A
  local W="$SB/$1"
  printf 'u1\n' > "$W/upstream/A/up.txt"; git -C "$W/upstream/A" add -A >/dev/null; git -C "$W/upstream/A" commit -qm "A: u1"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
  git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: bump A to u1"; git -C "$W/upstream" push -q origin main 2>/dev/null
  local n
  for n in $(seq 1 "$2"); do
    printf 's%s\n' "$n" > "$W/local/A/s$n.txt"; git -C "$W/local/A" add -A >/dev/null; GIT_AUTHOR_DATE="2026-02-0${n}T10:00:00+0000" git -C "$W/local/A" commit -qm "A: s$n"
    git -C "$W/local" add A >/dev/null; git -C "$W/local" commit -qm "seal s$n"
    if [ "$n" = 1 ]; then printf 'x\n' > "$W/local/x.txt"; git -C "$W/local" add x.txt; git -C "$W/local" commit -qm "unrelated"; fi
  done
}
subject_of() { git -C "$1" log -1 --format=%s "$2"; }

section "Several pointer commits of mine are kept one by one: each records the rewritten copy of what it pointed at"
multi_world p3 3
W="$SB/p3"
OUT=$(probe sync_plan "$W/local")
ok "the preview plans one rebase of A and says what each pointer commit will record" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['action'],x['predicted_gitlink_conflict'],'|',' / '.join(x['notes']))")" "True:rebase True | 3 commits of yours move its pointer; each will record the rewritten copy of the commit it pointed at"
OUT=$(probe sync_run "$W/local")
ok "the update finishes without a pause" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf headline)" "True:Updated: 4 commits of yours on top of 1 new commit from origin/main."
ok "  all four commits on top, in order" "$(subjects "$W/local" origin/main..HEAD)" "seal s3|seal s2|unrelated|seal s1|"
ok "  each pointer commit records the copy of what it pointed at" "$(for c in HEAD~3 HEAD~1 HEAD; do subject_of "$W/local/A" "$(git -C "$W/local" rev-parse "$c:A")"; done | tr '\n' '|')" "A: s1|A: s2|A: s3|"
ok "  on A's rebased line, which contains upstream's commit" "$(git -C "$W/local/A" log --format=%s | tr '\n' '|')" "A: s3|A: s2|A: s1|A: u1|A: initial|"
ok "  the branch records where A is and nothing is off its recorded commit" "$(git -C "$W/local" rev-parse HEAD:A):$(git -C "$W/local" submodule status | grep -c '^[+-]' || true)" "$(head_of "$W/local/A"):0"
ok "  every check passes, as previewed" "$(printf '%s' "$OUT" | jqf detail):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(all(d['sync']['verified'].values()))")" "As previewed: 4 commits of yours on top of 1 incoming.:True"
ok "  and A's outcome says rebased" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['path'],s['action']) for s in d['sync']['submodules']])")" "[('A', 'rebased')]"

section "Two pointer commits: the first one's commit is already upstream (a twin), so that pointer commit has nothing left"
multi_world pt 2
W="$SB/pt"
S1="$(git -C "$W/local/A" rev-parse HEAD~1)"
git -C "$W/upstream/A" fetch -q "$W/local/A" main; git -C "$W/upstream/A" cherry-pick "$S1" >/dev/null 2>&1; git -C "$W/upstream/A" push -q origin main 2>/dev/null
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: bump A to s1 (#9)"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "the preview still plans the rebase" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(d['submodules'][0]['action'])")" "True:rebase"
OUT=$(probe sync_run "$W/local")
ok "the update finishes" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD)" "True:seal s2|unrelated|"
ok "  the pointer commit whose commit upstream already had is named as such" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was dropped because upstream already contained it: seal s1"
ok "  the surviving pointer commit records s2's copy on top of upstream's s1" "$(subject_of "$W/local/A" "$(git -C "$W/local" rev-parse HEAD:A)"):$(git -C "$W/local/A" log --format=%s | tr '\n' '|')" "A: s2:A: s2|A: s1|A: u1|A: initial|"
ok "  nothing is off its recorded commit" "$(git -C "$W/local" submodule status | grep -c '^[+-]' || true)" "0"

section "Abort after the pointer stops puts both the branch and A back"
mini pa A
W="$SB/pa"; seed_file pa
printf 'u1\n' > "$W/upstream/A/up.txt"; git -C "$W/upstream/A" add -A >/dev/null; git -C "$W/upstream/A" commit -qm "A: u1"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
edit_line "$W/upstream/F.txt" 3 "(upstream)"; git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qam "super: bump A to u1, line 3"; git -C "$W/upstream" push -q origin main 2>/dev/null
for n in 1 2; do printf 's%s\n' "$n" > "$W/local/A/s$n.txt"; git -C "$W/local/A" add -A >/dev/null; git -C "$W/local/A" commit -qm "A: s$n"; git -C "$W/local" add A >/dev/null; git -C "$W/local" commit -qm "seal s$n"; [ "$n" = 1 ] && { printf 'x\n' > "$W/local/x.txt"; git -C "$W/local" add x.txt; git -C "$W/local" commit -qm "unrelated"; }; done
edit_line "$W/local/F.txt" 3 "(mine)"; git -C "$W/local" commit -qam "mine: line 3"
A0="$(head_of "$W/local/A")"; S0="$(head_of "$W/local")"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update pauses on F.txt after both pointer stops were handled" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths):$([ "$(head_of "$W/local/A")" != "$A0" ] && echo rebased || echo not)" "F.txt\"]:rebased"
OUT=$(probe sync_abort "$W/local")
ok "abort puts A and the branch back" "$(printf '%s' "$OUT" | jqf headline):$(head_of "$W/local/A"):$(head_of "$W/local")" "Update aborted.:$A0:$S0"
ok "  with both pointer commits as they were" "$(subjects "$W/local" origin/main..HEAD):$(git -C "$W/local" submodule status | grep -c '^[+-]' || true)" "mine: line 3|seal s2|unrelated|seal s1|:0"

section "A pointer of mine that is not on A's checked-out line keeps the blocker, saying why"
multi_world po 2
W="$SB/po"
# A's branch moved on by hand to a line that does not carry s1: my first pointer has no copy to record.
git -C "$W/local/A" reset -q --hard HEAD~2; printf 'other\n' > "$W/local/A/other.txt"; git -C "$W/local/A" add -A >/dev/null; git -C "$W/local/A" commit -qm "A: other line"
git -C "$W/local" add A >/dev/null; git -C "$W/local" commit -qm "seal other"
OUT=$(probe sync_plan "$W/local")
ok "the preview blocks with the reason" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);x=d['submodules'][0];print(x['remedy'],'|',x['reason'])")" "False:squash-own | 3 of your commits move this submodule's pointer and not every one points at a commit on its checked-out line; squash them, or do this one by hand"

section "A conflict inside A during its one rebase pauses; Continue records each pointer commit's copy"
multi_world pi 2
W="$SB/pi"
# Upstream's A also changes s1.txt's line, so my A: s1 conflicts when A is rebased.
printf 'upstream s1\n' > "$W/upstream/A/s1.txt"; git -C "$W/upstream/A" add -A >/dev/null; git -C "$W/upstream/A" commit -qm "A: u2 (s1.txt too)"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: bump A to u2"; git -C "$W/upstream" push -q origin main 2>/dev/null
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update pauses inside A" "$(printf '%s' "$OUT" | jqf headline)" "Update paused inside A."
printf 's1\n' > "$W/local/A/s1.txt"; git -C "$W/local/A" add s1.txt      # keep mine
OUT=$(probe sync_continue "$W/local")
ok "Continue finishes with both pointer commits on top" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD)" "True:seal s2|unrelated|seal s1|"
ok "  each recording the copy of what it pointed at" "$(for c in HEAD~2 HEAD; do subject_of "$W/local/A" "$(git -C "$W/local" rev-parse "$c:A")"; done | tr '\n' '|'):$(git -C "$W/local" submodule status | grep -c '^[+-]' || true)" "A: s1|A: s2|:0"

section "Record this pointer: one submodule at a time, every refusal named, the whole set still one call"
mini rp B C
W="$SB/rp"
# Both checkouts move ahead of what my branch records; I record B alone.
printf 'bmine\n' > "$W/local/B/bmine.txt"; git -C "$W/local/B" add -A >/dev/null; git -C "$W/local/B" commit -qm "B: mine"
printf 'cmine\n' > "$W/local/C/cmine.txt"; git -C "$W/local/C" add -A >/dev/null; git -C "$W/local/C" commit -qm "C: mine"
B1="$(head_of "$W/local/B")"; C1="$(head_of "$W/local/C")"; C_REC0="$(git -C "$W/local" rev-parse HEAD:C)"; S0="$(head_of "$W/local")"
OUT=$(probe record_pointers "$W/local" B)
ok "recording B alone makes one commit" "$(printf '%s' "$OUT" | jqf headline):$(git -C "$W/local" rev-list --count "$S0..HEAD")" "Recorded 1 submodule pointer(s).:1"
ok "  whose message names only B" "$(subjects "$W/local" -1)" "Record submodule pointers: B|"
ok "  and the result's detail is that message" "$(printf '%s' "$OUT" | jqf detail)" "Record submodule pointers: B"
ok "  B is recorded" "$(git -C "$W/local" rev-parse HEAD:B)" "$B1"
ok "  C's record is unchanged" "$(git -C "$W/local" rev-parse HEAD:C)" "$C_REC0"
ok "  and C is still unrecorded" "$(git -C "$W/local" submodule status -- C | cut -c1)" "+"
# The refusals, each a stable code and a sentence naming the path and why.
S1="$(head_of "$W/local")"
OUT=$(probe record_pointers "$W/local" B)
ok "B again: already recorded, so refused" "$(printf '%s' "$OUT" | jqf refusal.code)|$(printf '%s' "$OUT" | jqf refusal.message)" "not-ahead|B is checked out at $(printf '%s' "$B1" | cut -c1-7), the commit this branch records already; nothing to record."
git -C "$W/local/B" checkout -q --detach HEAD~1
OUT=$(probe record_pointers "$W/local" B)
ok "a checkout behind the record is refused, saying so" "$(printf '%s' "$OUT" | jqf refusal.code)|$(printf '%s' "$OUT" | jqf refusal.message)" "not-ahead|B is checked out at $(head_of "$W/local/B" | cut -c1-7) which is not ahead of the $(printf '%s' "$B1" | cut -c1-7) this branch records (it is behind it); nothing to record."
printf 'elsewhere\n' > "$W/local/B/elsewhere.txt"; git -C "$W/local/B" add -A >/dev/null; git -C "$W/local/B" commit -qm "B: elsewhere"
OUT=$(probe record_pointers "$W/local" B)
ok "a diverged checkout is refused, saying so" "$(printf '%s' "$OUT" | jqf refusal.code)|$(printf '%s' "$OUT" | jqf refusal.message)" "not-ahead|B is checked out at $(head_of "$W/local/B" | cut -c1-7) which is not ahead of the $(printf '%s' "$B1" | cut -c1-7) this branch records (the two have diverged); nothing to record."
git -C "$W/local/B" checkout -q main
OUT=$(probe record_pointers "$W/local" README.md)
ok "a path that is not a submodule is refused" "$(printf '%s' "$OUT" | jqf refusal.code)|$(printf '%s' "$OUT" | jqf refusal.message)" "not-a-submodule|README.md is not a submodule of this repository; nothing to record."
OUT=$(probe record_pointers "$W/local" "C,README.md")
ok "  one bad path refuses the whole request: C is not recorded on the side" "$(printf '%s' "$OUT" | jqf refusal.code):$(head_of "$W/local")" "not-a-submodule:$S1"
OUT=$(probe record_pointers "$W/local" "../upstream/B")
ok "a path outside the repository is refused" "$(printf '%s' "$OUT" | jqf refusal.code)|$(printf '%s' "$OUT" | jqf refusal.message)" "outside-repository|../upstream/B isn't a path inside this repository; nothing to record."
OUT=$(pinvoke changes_record_pointers "{\"repoPath\":\"$W/local\",\"paths\":[\"$W/local/C\"]}")
ok "  so is an absolute one, through the UI's own call" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "outside-repository"
OUT=$(pinvoke changes_record_pointers "{\"repoPath\":\"$W/local\",\"paths\":[]}")
ok "an empty list names nothing, and says so" "$(printf '%s' "$OUT" | jqf value.refusal.code):$(printf '%s' "$OUT" | jqf value.refusal.message)" "nothing-to-record:No submodule was named; nothing to record."
git -C "$W/local" submodule deinit -q -f -- B
OUT=$(probe record_pointers "$W/local" B)
ok "a submodule that is not populated is refused" "$(printf '%s' "$OUT" | jqf refusal.code)|$(printf '%s' "$OUT" | jqf refusal.message)" "not-populated|B is not initialised, so nothing is checked out there to record."
git -C "$W/local" submodule update -q --init -- B 2>/dev/null; git -C "$W/local/B" checkout -q main
# Then the other path, on its own.
OUT=$(probe record_pointers "$W/local" C)
ok "recording C makes one more commit, naming only C" "$(printf '%s' "$OUT" | jqf headline):$(subjects "$W/local" -1)" "Recorded 1 submodule pointer(s).:Record submodule pointers: C|"
ok "  C is recorded" "$(git -C "$W/local" rev-parse HEAD:C)" "$C1"
ok "  and nothing is left unrecorded" "$(git -C "$W/local" submodule status 2>/dev/null | grep -c '^[+-]' || true)" "0"
# The call without paths is unchanged: every submodule that is ahead, in one commit.
printf 'b3\n' > "$W/local/B/b3.txt"; git -C "$W/local/B" add -A >/dev/null; git -C "$W/local/B" commit -qm "B: mine 2"
printf 'c3\n' > "$W/local/C/c3.txt"; git -C "$W/local/C" add -A >/dev/null; git -C "$W/local/C" commit -qm "C: mine 2"
OUT=$(pinvoke changes_record_pointers "{\"repoPath\":\"$W/local\"}")
ok "the call without paths still records every submodule that is ahead, in one commit" "$(printf '%s' "$OUT" | jqf value.headline):$(subjects "$W/local" -1)" "Recorded 2 submodule pointer(s).:Record submodule pointers: B, C|"
ok "  both are recorded" "$(git -C "$W/local" rev-parse HEAD:B):$(git -C "$W/local" rev-parse HEAD:C)" "$(head_of "$W/local/B"):$(head_of "$W/local/C")"
# A record the submodule does not have yet (a superproject pull without a submodule
# fetch) cannot be compared, so it is refused by name. The clone comes first, and
# the pull is told not to fetch submodules on demand, or B would have the commit.
git clone -q --recurse-submodules "$W/super.git" "$W/stale" 2>/dev/null; git -C "$W/stale/B" checkout -q main
printf 'bup\n' > "$W/upstream/B/bup.txt"; git -C "$W/upstream/B" add -A >/dev/null; git -C "$W/upstream/B" commit -qm "B: upstream"; git -C "$W/upstream/B" push -q origin main 2>/dev/null
git -C "$W/upstream" add B >/dev/null; git -C "$W/upstream" commit -qm "super: bump B"; git -C "$W/upstream" push -q origin main 2>/dev/null
git -C "$W/stale" -c fetch.recurseSubmodules=no pull -q --ff-only 2>/dev/null
OUT=$(probe record_pointers "$W/stale" B)
ok "a recorded commit the submodule has not fetched is refused, saying what to do" "$(printf '%s' "$OUT" | jqf refusal.code)|$(printf '%s' "$OUT" | jqf refusal.message)" "recorded-missing|B is checked out at $(head_of "$W/stale/B" | cut -c1-7), but the $(git -C "$W/stale" rev-parse HEAD:B | cut -c1-7) this branch records is not in the submodule yet, so the two cannot be compared; fetch inside it first."

section "A pointer stop after a file pause records its own copy even when A was staged by hand meanwhile"
mini pz A
W="$SB/pz"; seed_file pz
printf 'u1\n' > "$W/upstream/A/up.txt"; git -C "$W/upstream/A" add -A >/dev/null; git -C "$W/upstream/A" commit -qm "A: u1"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
edit_line "$W/upstream/F.txt" 3 "(upstream)"; git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qam "super: bump A to u1, line 3"; git -C "$W/upstream" push -q origin main 2>/dev/null
seal() { printf 's%s\n' "$1" > "$W/local/A/s$1.txt"; git -C "$W/local/A" add -A >/dev/null; GIT_AUTHOR_DATE="2026-02-0$1T10:00:00+0000" git -C "$W/local/A" commit -qm "A: s$1"; git -C "$W/local" add A >/dev/null; git -C "$W/local" commit -qm "seal s$1"; }
seal 1
edit_line "$W/local/F.txt" 3 "(mine)"; git -C "$W/local" commit -qam "mine: line 3"
seal 2; seal 3
ok "the fixture: seal s1, mine: line 3, seal s2, seal s3" "$(subjects "$W/local" origin/main..HEAD)" "seal s3|seal s2|mine: line 3|seal s1|"
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update pauses on F.txt after the first pointer stop" "$(printf '%s' "$OUT" | jqf sync.needs_user.paths)" "F.txt"
probe tree_resolve_side "$W/local" "mine,F.txt" >/dev/null
git -C "$W/local" add --all --                                      # "Stage all" on the page: A, sitting at s3', goes in too
OUT=$(probe sync_continue "$W/local")
ok "Continue finishes with every commit on top" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD)" "True:seal s3|seal s2|mine: line 3|seal s1|"
ok "  seal s2 records s2's own copy, not upstream's pointer" "$(subject_of "$W/local/A" "$(git -C "$W/local" rev-parse HEAD~1:A)")" "A: s2"
ok "  and nothing is called already upstream" "$(printf '%s' "$OUT" | jqf detail | grep -c 'already contained' || true):$(printf '%s' "$OUT" | jqf sync.own_dropped)" "0:[]"

section "A pointer commit that also changes files is kept when its pointer turns out to be upstream's own"
multi_world pk 2
W="$SB/pk"
S1="$(git -C "$W/local/A" rev-parse HEAD~1)"; S2="$(git -C "$W/local/A" rev-parse HEAD)"
# Fold a file change into the first pointer commit, and give upstream a twin of A: s1.
git -C "$W/local" reset -q --hard origin/main 2>/dev/null; git -C "$W/local/A" reset -q --hard "$S1"
printf 'x\n' > "$W/local/x.txt"; git -C "$W/local" add x.txt A; git -C "$W/local" commit -qm "seal s1 (and x.txt)"
git -C "$W/local/A" reset -q --hard "$S2"; git -C "$W/local" add A; git -C "$W/local" commit -qm "seal s2"
git -C "$W/upstream/A" fetch -q "$W/local/A" main 2>/dev/null; git -C "$W/upstream/A" cherry-pick "$S1" >/dev/null 2>&1; git -C "$W/upstream/A" push -q origin main 2>/dev/null
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: bump A to s1 (#7)"; git -C "$W/upstream" push -q origin main 2>/dev/null
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update finishes with both commits on top: the file change kept the first one" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD):$([ -e "$W/local/x.txt" ] && echo x || echo nox)" "True:seal s2|seal s1 (and x.txt)|:x"
ok "  its pointer is upstream's own, and the result does not call the commit dropped" "$(git -C "$W/local" rev-parse HEAD~1:A | cut -c1-7):$(git -C "$W/local" rev-parse origin/main:A | cut -c1-7):$(printf '%s' "$OUT" | jqf sync.own_dropped)" "$(git -C "$W/local" rev-parse origin/main:A | cut -c1-7):$(git -C "$W/local" rev-parse origin/main:A | cut -c1-7):[]"

section "A pointer commit whose target is the tip, and a twin of that tip upstream: dropped because upstream already had it"
multi_world pw 2
W="$SB/pw"
S2="$(git -C "$W/local/A" rev-parse HEAD)"
git -C "$W/upstream/A" fetch -q "$W/local/A" main 2>/dev/null; git -C "$W/upstream/A" cherry-pick "$S2" >/dev/null 2>&1; git -C "$W/upstream/A" push -q origin main 2>/dev/null
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: bump A to s2 (#8)"; git -C "$W/upstream" push -q origin main 2>/dev/null
probe sync_plan "$W/local" >/dev/null
OUT=$(probe sync_run "$W/local")
ok "the update finishes; seal s2's pointer is upstream's twin of s2" "$(printf '%s' "$OUT" | jqf ok):$(subjects "$W/local" origin/main..HEAD)" "True:unrelated|seal s1|"
ok "  and that commit is named as already upstream, not as resolved into nothing" "$(printf '%s' "$OUT" | jqf detail)" "1 commit of yours was dropped because upstream already contained it: seal s2"

section "A submodule whose address changed upstream is fetched from the new one, and the update re-points it"
mini pu A
W="$SB/pu"
# Upstream moves A to another address: a copy of its repository with a commit only that copy has.
git clone -q --bare "$W/A.git" "$W/A-moved.git" 2>/dev/null
git clone -q "$W/A-moved.git" "$W/author-A2" 2>/dev/null; printf 'moved\n' > "$W/author-A2/moved.txt"; git -C "$W/author-A2" add -A >/dev/null; git -C "$W/author-A2" commit -qm "A: only at the new address"; git -C "$W/author-A2" push -q origin main 2>/dev/null
git -C "$W/upstream" config -f .gitmodules submodule.A.url "$W/A-moved.git"; git -C "$W/upstream" submodule sync -q -- A 2>/dev/null
git -C "$W/upstream/A" pull -q --ff-only origin main 2>/dev/null
git -C "$W/upstream" add .gitmodules A >/dev/null; git -C "$W/upstream" commit -qm "super: move A to its new address"; git -C "$W/upstream" push -q origin main 2>/dev/null
OUT=$(probe sync_plan "$W/local")
ok "the preview sees the move and fetched the new commit from there — no fetch-failed blocker" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);s=[s for s in d['submodules'] if s['path']=='A'][0];print(d['can_run'], s['action'], s['remedy'], s['url_change']['to'].endswith('/A-moved.git'), s['url_change']['from'].endswith('/A.git'))")" "True fast-forward None True True"
ok "  and says what the update will do about it" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);s=[s for s in d['submodules'] if s['path']=='A'][0];print([n for n in s['notes'] if 'upstream moved it' in n][0])")" "upstream moved it: it now lives at $W/A-moved.git (it fetched from $W/A.git until now); the update re-points it there, as \`git submodule sync\` does, before aligning it"
ok "  the preview changed no address" "$(git -C "$W/local/A" remote get-url origin)" "$W/A.git"
ok "  what it fetched from the new address is parked, and the remote-tracking branch is left alone" "$(git -C "$W/local/A" for-each-ref --format='%(refname)' refs/gitswitch-moved/ | head -1):$(git -C "$W/local/A" rev-parse origin/main)" "refs/gitswitch-moved/origin/main:$(git -C "$W/author-A" rev-parse HEAD)"
OUT=$(probe sync_run "$W/local")
ok "the update finishes with A at upstream's commit" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$W/local/A" rev-parse HEAD)" "True:$(git -C "$W/local" rev-parse HEAD:A)"
ok "  A fetches from the new address now, in its own config and the superproject's" "$(git -C "$W/local/A" remote get-url origin):$(git -C "$W/local" config submodule.A.url)" "$W/A-moved.git:$W/A-moved.git"
ok "  its remote-tracking branch comes from there, and the parking is cleared" "$(git -C "$W/local/A" rev-parse origin/main):$(git -C "$W/local/A" for-each-ref refs/gitswitch-moved/ | wc -l | tr -d ' ')" "$(git -C "$W/local" rev-parse HEAD:A):0"
ok "  and the result says so" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin)['sync'];print([s['note'] for s in d['submodules'] if s['path']=='A'][0])")" "fast-forwarded to $(git -C "$W/local" rev-parse --short=7 HEAD:A); now fetches from $W/A-moved.git"

if git lfs version >/dev/null 2>&1; then
section "Large files: the update writes pointer stubs and downloads afterwards — an LFS server that does not answer does not fail it"
# Left to itself git downloads large files INSIDE the rebase, through the
# smudge filter, on the rebase's clock: a slow or unreachable LFS server
# failed the whole update. Pull already avoids that; the update now does too.
L="$SB/lfsw"; mkdir -p "$L"
git init -q --bare "$L/super.git"; git clone -q "$L/super.git" "$L/author" 2>/dev/null
( cd "$L/author" && git lfs install --local >/dev/null 2>&1 && git lfs track "*.bin" >/dev/null )
head -c 4096 /dev/urandom > "$L/author/model.bin"; head -c 3000 /dev/urandom > "$L/author/other.bin"; printf 'readme\n' > "$L/author/README.md"
git -C "$L/author" add -A >/dev/null; git -C "$L/author" commit -qm "super: initial"; git -C "$L/author" push -q origin main 2>/dev/null
for c in down up dirty; do
  ( cd "$L" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$L/super.git" "$c" 2>/dev/null ); probe lfs_pull "$L/$c" >/dev/null
  printf '%s\n' "$c" > "$L/$c/mine.txt"; git -C "$L/$c" add -A >/dev/null; git -C "$L/$c" commit -qm "mine: $c"
done
ok "(each clone starts with every large file present)" "$(for c in down up dirty; do probe lfs_status "$L/$c" | jqf pointers; done | tr '\n' ',')" "0,0,0,"
head -c 5120 /dev/urandom > "$L/author/model.bin"; git -C "$L/author" commit -qam "upstream: a new model"; git -C "$L/author" push -q origin main 2>/dev/null
# The LFS server stops answering: its objects are moved away.
mv "$L/super.git/lfs" "$L/lfs-away"
probe sync_plan "$L/down" >/dev/null; OUT=$(probe sync_run "$L/down")
ok "an update whose large file cannot be downloaded right now still updates" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf headline)" "True:Updated: 1 commit of yours on top of 1 new commit"
ok "  your commit is on top of upstream's" "$(subjects "$L/down" HEAD~2..HEAD)" "mine: down|upstream: a new model|"
ok "  the new large file is a pointer stub for now" "$([ "$(wc -c < "$L/down/model.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
ok "  and the result says how to fetch it" "$(printf '%s' "$OUT" | jqf detail)" "The large files here didn't download"
ok "  nothing is half-written: the working tree is clean, no rebase is left" "$(git -C "$L/down" status --porcelain | wc -l | tr -d ' '):$([ -d "$(git -C "$L/down" rev-parse --git-path rebase-merge)" ] && echo rebasing || echo none)" "0:none"
# git-lfs made a fresh, empty store meanwhile; the real one goes back in its place.
rm -rf "$L/super.git/lfs"; mv "$L/lfs-away" "$L/super.git/lfs"
probe lfs_pull "$L/down" >/dev/null
ok "  once the server answers, the LFS card brings it" "$(wc -c < "$L/down/model.bin" | tr -d ' '):$(probe lfs_status "$L/down" | jqf pointers)" "5120:0"
# The server answers: the large file is downloaded after the rebase, not inside it.
probe sync_plan "$L/up" >/dev/null; OUT=$(probe sync_run "$L/up")
ok "with the server answering, the update downloads the new large file afterwards and says so" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf detail)" "Downloaded 1 LFS file here."
ok "  it is content, and every large file is present again" "$(wc -c < "$L/up/model.bin" | tr -d ' '):$(probe lfs_status "$L/up" | jqf pointers)" "5120:0"
# One of your own uncommitted changes is a large file: the update stashes it
# and puts it back, so it must come back as itself — never as a stub.
head -c 6000 /dev/urandom > "$L/mine-other.bin"; cp "$L/mine-other.bin" "$L/dirty/other.bin"
probe sync_plan "$L/dirty" >/dev/null; OUT=$(probe sync_run "$L/dirty")
ok "an uncommitted change to a large file survives the update as itself, not as a pointer stub" "$(printf '%s' "$OUT" | jqf ok):$(cmp -s "$L/mine-other.bin" "$L/dirty/other.bin" && echo same || echo "changed: $(wc -c < "$L/dirty/other.bin" | tr -d ' ') bytes")" "True:same"
ok "  and upstream's new large file arrived as content" "$(wc -c < "$L/dirty/model.bin" | tr -d ' ')" "5120"
# A deliberately partial checkout: only other.bin was downloaded. Upstream
# changes other.bin — a file this person HAS — so it must arrive as content,
# not be turned into a stub that the download afterwards cannot tell apart
# from the files left out on purpose.
( cd "$L" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$L/super.git" partial 2>/dev/null ); probe lfs_pull_paths "$L/partial" "other.bin" >/dev/null
printf 'partial\n' > "$L/partial/mine.txt"; git -C "$L/partial" add -A >/dev/null; git -C "$L/partial" commit -qm "mine: partial"
ok "(the partial clone holds other.bin, and model.bin as a stub)" "$(wc -c < "$L/partial/other.bin" | tr -d ' '):$([ "$(wc -c < "$L/partial/model.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "3000:stub"
head -c 3500 /dev/urandom > "$L/author/other.bin"; git -C "$L/author" commit -qam "upstream: a new other"; git -C "$L/author" push -q origin main 2>/dev/null
probe sync_plan "$L/partial" >/dev/null; OUT=$(probe sync_run "$L/partial")
ok "in a partial checkout, a large file you had and upstream changed arrives as content" "$(printf '%s' "$OUT" | jqf ok):$(wc -c < "$L/partial/other.bin" | tr -d ' ')" "True:3500"
# A clone made where git-lfs was not set up: no LFS filters, every large file
# a pointer stub. Without the page's "Also download Git LFS files" the update
# says so; with it, the update downloads them afterwards, as Pull does.
for c in stubs stubs2 stubs3; do
  ( cd "$L" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$L/super.git" "$c" 2>/dev/null )
  printf '%s\n' "$c" > "$L/$c/mine.txt"; git -C "$L/$c" add -A >/dev/null; git -C "$L/$c" commit -qm "mine: $c"
done
ok "(a clone without LFS filters holds only pointer stubs)" "$(probe lfs_status "$L/stubs" | "$PY" -c 'import sys,json;d=json.load(sys.stdin);print(d["filters_configured"], d["pointers"] > 0)')" "False True"
head -c 7000 /dev/urandom > "$L/author/new.bin"; git -C "$L/author" add -A >/dev/null; git -C "$L/author" commit -qm "upstream: new.bin"; git -C "$L/author" push -q origin main 2>/dev/null
probe sync_plan "$L/stubs" >/dev/null; OUT=$(probe sync_run "$L/stubs")
ok "without the option the update says which large files are still pointer stubs" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf detail)" "pointer stubs — use Pull LFS files"
probe sync_plan "$L/stubs2" >/dev/null; OUT=$(probe sync_run "$L/stubs2" "lfs")
ok "with the option the update downloads them afterwards" "$(printf '%s' "$OUT" | jqf ok):$(wc -c < "$L/stubs2/new.bin" | tr -d ' '):$(probe lfs_status "$L/stubs2" | jqf pointers)" "True:7000:0"
ok "  and says so" "$(printf '%s' "$OUT" | jqf detail)" "Downloaded"
# The LFS server stops answering: the update still updates, and says the download did not happen.
mv "$L/super.git/lfs" "$L/lfs-away"
probe sync_plan "$L/stubs3" >/dev/null; OUT=$(probe sync_run "$L/stubs3" "lfs")
ok "with the option and the LFS server away, the update still updates" "$(printf '%s' "$OUT" | jqf ok):$([ -d "$(git -C "$L/stubs3" rev-parse --git-path rebase-merge)" ] && echo rebasing || echo none):$(git -C "$L/stubs3" status --porcelain | wc -l | tr -d ' ')" "True:none:0"
ok "  and says the large files did not download" "$(printf '%s' "$OUT" | jqf detail)" "didn't download"
rm -rf "$L/super.git/lfs"; mv "$L/lfs-away" "$L/super.git/lfs"
# An update that finished, with its state left behind (a crash between
# finishing and tidying up — the window a long download used to widen):
# never shown as a paused update, never undone by an Abort. A real state is
# made by a paused update, then marked as finished.
printf 'theirs\n' > "$L/author/README.md"; git -C "$L/author" commit -qam "upstream: readme"; git -C "$L/author" push -q origin main 2>/dev/null
printf 'mine\n' > "$L/up/README.md"; git -C "$L/up" commit -qam "mine: readme"
probe sync_plan "$L/up" >/dev/null; probe sync_run "$L/up" >/dev/null
has_sync() { probe status "$1" | "$PY" -c 'import sys,json;print(json.load(sys.stdin).get("sync") is not None)'; }
ok "(control: a paused update is shown as one)" "$(has_sync "$L/up")" "True"
STATE_FILE="$(git -C "$L/up" rev-parse --path-format=absolute --git-path gitswitch-sync.json)"
"$PY" - "$STATE_FILE" <<'PYST'
import json, sys
st = json.load(open(sys.argv[1])); st["phase"] = "done"; st["needs_user"] = None
json.dump(st, open(sys.argv[1], "w"))
PYST
UPHEAD="$(head_of "$L/up")"
ok "a finished update's leftover state is not shown as a paused one" "$(has_sync "$L/up")" "False"
ok "  and Abort refuses it rather than undoing the finished work" "$(probe sync_abort "$L/up" | jqf refusal.code):$(head_of "$L/up")" "no-sync:$UPHEAD"
git -C "$L/up" rebase --abort 2>/dev/null; rm -f "$STATE_FILE"
fi

section "Large files: each repository of an update is decided alone"
# One submodule whose large files are pointer stubs (a clone that never fetched
# them) does not send the superproject's own large files back into its rebase,
# where an LFS server that does not answer stops the whole update.
LS="$SB/lfsub"; mkdir -p "$LS"
git init -q --bare "$LS/lib.git"; git clone -q "$LS/lib.git" "$LS/lib-author" 2>/dev/null
( cd "$LS/lib-author" && git lfs install --local >/dev/null 2>&1 && git lfs track "*.bin" >/dev/null )
head -c 2000 /dev/urandom > "$LS/lib-author/lib.bin"; git -C "$LS/lib-author" add -A >/dev/null; git -C "$LS/lib-author" commit -qm "lib: initial"; git -C "$LS/lib-author" push -q origin main 2>/dev/null
git init -q --bare "$LS/super.git"; git clone -q "$LS/super.git" "$LS/author" 2>/dev/null
( cd "$LS/author" && git lfs install --local >/dev/null 2>&1 && git lfs track "*.bin" >/dev/null )
head -c 4096 /dev/urandom > "$LS/author/model.bin"; head -c 3000 /dev/urandom > "$LS/author/other.bin"
( cd "$LS/author" && GIT_LFS_SKIP_SMUDGE=1 git submodule add -q "$LS/lib.git" lib 2>/dev/null )
git -C "$LS/author" add -A >/dev/null; git -C "$LS/author" commit -qm "super: initial"; git -C "$LS/author" push -q origin main 2>/dev/null
( cd "$LS" && GIT_LFS_SKIP_SMUDGE=1 git clone -q --recurse-submodules "$LS/super.git" w 2>/dev/null )
probe lfs_pull "$LS/w" >/dev/null
printf 'w\n' > "$LS/w/mine.txt"; git -C "$LS/w" add mine.txt >/dev/null; git -C "$LS/w" commit -qm "mine: w"
ok "(the superproject's large files are all here; the submodule's are pointer stubs)" "$(probe lfs_status "$LS/w" | jqf pointers):$(probe lfs_status "$LS/w/lib" | jqf pointers)" "0:1"
head -c 5120 /dev/urandom > "$LS/author/model.bin"; git -C "$LS/author" commit -qam "upstream: a new model"; git -C "$LS/author" push -q origin main 2>/dev/null
mv "$LS/super.git/lfs" "$LS/lfs-away"
probe sync_plan "$LS/w" >/dev/null; OUT=$(probe sync_run "$LS/w")
ok "with the LFS server away the update still updates: the submodule's stubs do not make the superproject's rebase download" "$(printf '%s' "$OUT" | jqf ok):$([ -d "$(git -C "$LS/w" rev-parse --git-path rebase-merge)" ] && echo rebasing || echo none)" "True:none"
ok "  the new large file is a pointer stub for now" "$([ "$(wc -c < "$LS/w/model.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
ok "  and the result says so" "$(printf '%s' "$OUT" | jqf detail)" "The large files here didn't download"
ok "  and names the submodule's stubs too" "$(printf '%s' "$OUT" | jqf detail)" "1 LFS file in lib is still a pointer stub"
rm -rf "$LS/super.git/lfs"; mv "$LS/lfs-away" "$LS/super.git/lfs"
# A repository whose LFS filter runs although filter.lfs.required is not set
# is decided like any other: stubbed in the rebase, downloaded afterwards.
( cd "$LS" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$LS/super.git" r 2>/dev/null ); probe lfs_pull "$LS/r" >/dev/null
git -C "$LS/r" config --unset filter.lfs.required
printf 'r\n' > "$LS/r/mine.txt"; git -C "$LS/r" add mine.txt >/dev/null; git -C "$LS/r" commit -qm "mine: r"
head -c 4500 /dev/urandom > "$LS/author/model.bin"; git -C "$LS/author" commit -qam "upstream: a model for r"; git -C "$LS/author" push -q origin main 2>/dev/null
probe sync_plan "$LS/r" >/dev/null; OUT=$(probe sync_run "$LS/r")
ok "a repository whose LFS filter runs without filter.lfs.required set is stubbed and downloaded afterwards, like any other" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf detail)" "Downloaded 1 LFS file here."
ok "  and its large file is content" "$(wc -c < "$LS/r/model.bin" | tr -d ' ')" "4500"
# Where git does download inside a rewrite (a large file among your own
# uncommitted changes), it does so on the large-file clock, with git-lfs's
# progress in the file the page reads — seen by the git-lfs git runs.
LFS_BIN="$(command -v git-lfs)"; mkdir -p "$LS/wrap"
cat > "$LS/wrap/git-lfs" <<WRAP
#!/bin/sh
if [ "\$1" = "filter-process" ]; then
  case "\$(ps -o args= -p \$PPID 2>/dev/null)" in
    *" rebase "*) echo "rebase progress=\${GIT_LFS_PROGRESS:-none}" >> "$LS/lfs-env.log" ;;
  esac
fi
exec "$LFS_BIN" "\$@"
WRAP
chmod +x "$LS/wrap/git-lfs"
( cd "$LS" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$LS/super.git" d 2>/dev/null ); probe lfs_pull "$LS/d" >/dev/null
printf 'd\n' > "$LS/d/mine.txt"; git -C "$LS/d" add mine.txt >/dev/null; git -C "$LS/d" commit -qm "mine: d"
head -c 6000 /dev/urandom > "$LS/d/other.bin"
head -c 5500 /dev/urandom > "$LS/author/model.bin"; git -C "$LS/author" commit -qam "upstream: another model"; git -C "$LS/author" push -q origin main 2>/dev/null
rm -f "$LS/lfs-env.log"
probe sync_plan "$LS/d" >/dev/null; OUT=$(PATH="$LS/wrap:$PATH" probe sync_run "$LS/d")
ok "(an uncommitted large file: git downloads inside the rebase, and the update still finishes)" "$(printf '%s' "$OUT" | jqf ok)" "True"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) echo "  SKIP the git-lfs inside the rebase writes its progress where the page reads it (Git Bash's ps cannot name the caller)" ;;
  *) ok "  the git-lfs inside the rebase writes its progress where the page reads it" "$(sort -u "$LS/lfs-env.log" 2>/dev/null | tr '\\' '/' | tr '\n' '|')" "rebase progress=$LS/d/.git/gitswitch-lfs.progress|" ;;
esac
ok "  and upstream's new large file arrived as content" "$(wc -c < "$LS/d/model.bin" | tr -d ' ')" "5500"

section "Two pointer commits of mine that upstream's own pointer has passed are left out by git, not blocked"
mini sp A
W="$SB/sp"
for n in one two; do printf '%s\n' "$n" >> "$W/local/A/A.txt"; git -C "$W/local/A" commit -qam "A: my $n"; git -C "$W/local" add A >/dev/null; git -C "$W/local" commit -qm "bump A to $n"; done
# A's commits reach its remote, and upstream bumps A to a commit past both.
git -C "$W/local/A" push -q origin main 2>/dev/null
git -C "$W/upstream/A" pull -q --ff-only origin main 2>/dev/null
printf 'three\n' > "$W/upstream/A/three.txt"; git -C "$W/upstream/A" add -A >/dev/null; git -C "$W/upstream/A" commit -qm "A: three"; git -C "$W/upstream/A" push -q origin main 2>/dev/null
git -C "$W/upstream" add A >/dev/null; git -C "$W/upstream" commit -qm "super: bump A to three"; git -C "$W/upstream" push -q origin main 2>/dev/null
THREE="$(head_of "$W/upstream/A")"
OUT=$(probe sync_plan "$W/local")
ok "the preview does not block: both of my pointers are behind upstream's" "$(printf '%s' "$OUT" | jqf can_run):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print([(s['action'], s['remedy']) for s in d['submodules']])")" "True:[('fast-forward', None)]"
ok "  it foresees git leaving both pointer commits out" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(sum(1 for c in d['superproject']['own_commits'] if c['already_upstream']))"):$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(d['submodules'][0]['notes'])")" "2:"
ok "  and says so" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);print(d['submodules'][0]['notes'])")" "2 pointer commits of yours point at commits upstream's pointer already contains; git leaves them out"
OUT=$(probe sync_run "$W/local")
ok "the update finishes: A at upstream's pointer, nothing of mine left to replay" "$(printf '%s' "$OUT" | jqf ok):$(head_of "$W/local/A"):$(subjects "$W/local" origin/main..HEAD)" "True:$THREE:"
ok "  nothing is off its recorded commit" "$(git -C "$W/local" submodule status | grep -c '^[+-]' || true)" "0"

verify_result
