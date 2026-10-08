#!/bin/bash
# Sandbox suite for the Changes page operations, against real git.
# Throwaway repos, an isolated HOME, a bare repo on local disk as the "remote".
# No network, no GitHub accounts, no SSH.
set -u
source "$(dirname "$0")/lib.sh"
verify_init sandbox "Sandbox User" "sandbox@example.test"

# --- setup: bare "remote" + a clone ------------------------------------------
git init -q --bare "$SB/remote.git"
git clone -q "$SB/remote.git" "$SB/work" 2>/dev/null
cd "$SB/work"
git config user.name "Sandbox User"; git config user.email "sandbox@example.test"
echo "line1" > a.txt; echo "keep" > keep.txt
git add -A >/dev/null; git commit -qm "initial"; git push -q -u origin main 2>/dev/null

section "Staging files with awkward names"
printf 'changed\n' >> a.txt
printf 'new\n' > "my file.txt"
printf 'nl\n' > "$(printf 'weird\nname.txt')"
OUT=$(probe status "$SB/work")
ok "status sees 1 modified + 2 untracked" "$(printf '%s' "$OUT" | jqf unstaged_count),$(printf '%s' "$OUT" | jqf untracked_count)" "1,2"
OUT=$(probe stage "$SB/work" "my file.txt")
ok "stages a filename containing a space" "$(printf '%s' "$OUT" | jqf headline)" "Staged."
ok "  and it is staged now" "$(git -C "$SB/work" diff --cached --name-only)" "my file.txt"
OUT=$(probe stage "$SB/work" "$(printf 'weird\nname.txt')")
ok "stages a filename containing a newline" "$(git -C "$SB/work" diff --cached --name-only -z | tr '\0' '|')" "weird"
OUT=$(probe unstage "$SB/work" "my file.txt")
ok "unstages it again" "$(git -C "$SB/work" diff --cached --name-only)" "weird"
ok "  the space file left the index" "$(git -C "$SB/work" diff --cached --name-only | grep -c 'my file' || true)" "0"

section "Staging refuses to guess"
OUT=$(probe stage "$SB/work" "")
ok "empty selection is an error, never 'everything'" "$OUT" "No files selected"
OUT=$(probe stage "$SB/work" "../../../etc/passwd")
ok "a path outside the worktree is rejected" "$OUT" "isn't a path inside this repository"

section "Discard"
git -C "$SB/work" reset -q
printf 'junk\n' > "$SB/work/junk.txt"
printf 'more\n' >> "$SB/work/keep.txt"
OUT=$(probe discard "$SB/work" "keep.txt,junk.txt")
ok "discards tracked and untracked together" "$(printf '%s' "$OUT" | jqf headline)" "Discarded."
ok "  tracked file went back to HEAD" "$(cat "$SB/work/keep.txt")" "keep"
ok "  untracked file was deleted" "$([ -e "$SB/work/junk.txt" ] && echo present || echo gone)" "gone"
printf 'secret\n' > "$SB/work/.env"
mkdir -p "$SB/work/node_modules"; printf 'x\n' > "$SB/work/node_modules/dep.js"
printf 'ignored\n' > "$SB/work/.gitignore"; echo ".env" >> "$SB/work/.gitignore"; echo "node_modules/" >> "$SB/work/.gitignore"
printf 'touched\n' >> "$SB/work/a.txt"
OUT=$(probe discard "$SB/work" "a.txt")
ok "discarding one file leaves ignored files alone (never clean -x)" "$([ -e "$SB/work/.env" ] && echo kept || echo DELETED)" "kept"
ok "  node_modules survives too" "$([ -e "$SB/work/node_modules/dep.js" ] && echo kept || echo DELETED)" "kept"
OUT=$(probe discard "$SB/work" "")
ok "discard with no paths is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-paths"

section "Commit"
git -C "$SB/work" checkout -q -- . 2>/dev/null; rm -f "$SB/work/weird"*
printf 'feature\n' >> "$SB/work/a.txt"
OUT=$(probe commit "$SB/work" "no staging yet")
ok "refuses with nothing staged" "$(printf '%s' "$OUT" | jqf refusal.code)" "nothing-staged"
probe stage "$SB/work" "a.txt" >/dev/null
OUT=$(probe commit "$SB/work" "")
ok "refuses an empty message" "$(printf '%s' "$OUT" | jqf refusal.code)" "empty-message"
OUT=$(probe commit "$SB/work" "-starts with a dash and is long enough to matter")
ok "a message starting with '-' is not read as an option" "$(printf '%s' "$OUT" | jqf headline)" "Committed"
ok "  the subject survived verbatim" "$(git -C "$SB/work" log -1 --format=%s)" "-starts with a dash"
ok "  it reports the identity that actually landed" "$(printf '%s' "$OUT" | jqf detail)" "sandbox@example.test"

section "Commit runs hooks (the identity guard still works)"
cat > "$SB/work/.git/hooks/pre-commit" <<'HOOK'
#!/bin/sh
# >>> GitSwitch identity guard >>>
expected="someone.else@corp.test"
actual="$(git config user.email)"
if [ "$actual" != "$expected" ]; then
  echo "GitSwitch: commit blocked."
  echo "  this folder should commit as: $expected"
  exit 1
fi
HOOK
chmod +x "$SB/work/.git/hooks/pre-commit"
printf 'again\n' >> "$SB/work/a.txt"; probe stage "$SB/work" "a.txt" >/dev/null
OUT=$(probe commit "$SB/work" "should be blocked by the guard")
ok "a mismatched identity is refused by the hook" "$(printf '%s' "$OUT" | jqf advice.action)" "fix-identity"
ok "  and the app never suggests --no-verify" "$(printf '%s' "$OUT" | grep -c -- '--no-verify' || true)" "0"
ok "  nothing was committed" "$(git -C "$SB/work" log -1 --format=%s)" "-starts with a dash"
rm "$SB/work/.git/hooks/pre-commit"
OUT=$(probe commit "$SB/work" "second commit")
ok "commits again once the guard is gone" "$(printf '%s' "$OUT" | jqf headline)" "Committed"

section "Amend"
OUT=$(probe commit "$SB/work" "amended subject,amend")
ok "amends an unpushed commit" "$(printf '%s' "$OUT" | jqf headline)" "Amended"
ok "  the subject changed" "$(git -C "$SB/work" log -1 --format=%s)" "amended subject"
git -C "$SB/work" push -q origin main 2>/dev/null
OUT=$(probe commit "$SB/work" "try to amend a pushed commit,amend")
ok "refuses to amend a commit that is already on the remote" "$(printf '%s' "$OUT" | jqf refusal.code)" "already-pushed"
ok "  and says why in plain words" "$(printf '%s' "$OUT" | jqf refusal.message)" "force-push"

section "Push"
printf 'p1\n' >> "$SB/work/a.txt"; probe stage "$SB/work" "a.txt" >/dev/null
probe commit "$SB/work" "to push" >/dev/null
OUT=$(probe push "$SB/work" "")
ok "pushes and reports the ref that moved" "$(printf '%s' "$OUT" | jqf headline)" "Pushed to origin"
ok "  the remote really advanced" "$(git -C "$SB/remote.git" log -1 --format=%s main)" "to push"
OUT=$(probe push "$SB/work" "")
ok "a second push says up to date" "$(printf '%s' "$OUT" | jqf headline)" "Already up to date"

section "A branch started from main with tracking is never pushed into main"
# `git switch -c feature --track origin/main` leaves feature tracking main.
# Pushing "to the upstream" would land the feature's commit in main. git's own
# default (push.default=simple) refuses exactly this, and so does the app —
# offering to publish the branch under its own name instead.
git -C "$SB/work" switch -q -c feature --track origin/main 2>/dev/null
printf 'f1\n' > "$SB/work/feature.txt"; git -C "$SB/work" add -A >/dev/null; git -C "$SB/work" commit -qm "feature work"
OUT=$(probe push "$SB/work" "")
ok "the push is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "upstream-name-differs"
ok "  saying where it would have gone" "$(printf '%s' "$OUT" | jqf refusal.message)" "origin/main"
ok "  and offering the way out" "$(printf '%s' "$OUT" | jqf refusal.message)" "origin/feature"
ok "  main on the remote did not move" "$(git -C "$SB/remote.git" log -1 --format=%s main)" "to push"
OUT=$(probe push "$SB/work" "upstream")
ok "publishing sends it under its own name" "$(printf '%s' "$OUT" | jqf headline)" "Pushed to origin"
ok "  the remote has the branch" "$(git -C "$SB/remote.git" log -1 --format=%s feature)" "feature work"
ok "  which the branch now tracks" "$(git -C "$SB/work" rev-parse --abbrev-ref '@{u}')" "origin/feature"
ok "  and main is still untouched" "$(git -C "$SB/remote.git" log -1 --format=%s main)" "to push"
OUT=$(probe push "$SB/work" "")
ok "  a plain push now goes to its own branch" "$(printf '%s' "$OUT" | jqf headline)" "Already up to date"

section "A branch tracking a local branch has nowhere remote to go"
git -C "$SB/work" switch -q -c local-child --track main 2>/dev/null
OUT=$(probe push "$SB/work" "")
ok "the push is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "local-upstream"
ok "  naming the local branch" "$(printf '%s' "$OUT" | jqf refusal.message)" '`main`'
ok "  pulling is refused for the same reason, not sent to a remote called main" "$(probe pull "$SB/work" "ff-only" | jqf refusal.code)" "local-upstream"
OUT=$(probe push "$SB/work" "upstream")
ok "publishing it works" "$(printf '%s' "$OUT" | jqf headline)" "Pushed to origin"
ok "  and it tracks the remote now" "$(git -C "$SB/work" rev-parse --abbrev-ref '@{u}')" "origin/local-child"
git -C "$SB/work" switch -q main; git -C "$SB/work" branch -q -D feature local-child

section "Push blocking holds in a plain terminal"
OUT=$(probe block "$SB/work" "on")
ok "block reports itself as on" "$(printf '%s' "$OUT" | jqf repo_blocked)" "True"
ok "  the rewrite provably applies to origin" "$(git -C "$SB/work" remote get-url --push origin)" "gitswitch-push-blocked://"
ok "  a pre-push hook is installed" "$(head -3 "$SB/work/.git/hooks/pre-push" | tr '\n' ' ')" "GitSwitch push guard"
printf 'p2\n' >> "$SB/work/a.txt"; git -C "$SB/work" add -A >/dev/null; git -C "$SB/work" commit -qm "blocked push test"
TERM_PUSH=$(git -C "$SB/work" push origin main 2>&1; echo "rc=$?")
ok "a terminal push fails" "$TERM_PUSH" "gitswitch-push-blocked"
ok "  with a non-zero exit code" "$TERM_PUSH" "rc=128"
TERM_PUSH_NV=$(git -C "$SB/work" push --no-verify origin main 2>&1; echo "rc=$?")
ok "even with --no-verify (the URL rewrite is not a hook)" "$TERM_PUSH_NV" "gitswitch-push-blocked"
ok "  and the remote did not move" "$(git -C "$SB/remote.git" log -1 --format=%s main)" "to push"
OUT=$(probe push "$SB/work" "")
ok "the app refuses before running git at all" "$(printf '%s' "$OUT" | jqf refusal.code)" "push-blocked"
OUT=$(probe status "$SB/work")
ok "  status discloses the --no-verify gap honestly" "$(printf '%s' "$OUT" | jqf push.gaps)" "guard-rail, not a lock"

section "The hook catches what the URL rewrite misses"
git -C "$SB/work" remote add alias "sandbox-alias:owner/repo.git"
OUT=$(probe block "$SB/work" "on")
ok "re-applying derives a prefix for the ssh-alias remote" "$(git -C "$SB/work" config --local --get-all 'url.gitswitch-push-blocked://.pushInsteadOf' | tr '\n' ' ')" "sandbox-alias:"
ok "  so the alias remote is rewritten too" "$(git -C "$SB/work" remote get-url --push alias)" "gitswitch-push-blocked://"

section "Unblocking restores everything"
OUT=$(probe block "$SB/work" "off")
ok "block reports itself as off" "$(printf '%s' "$OUT" | jqf repo_blocked)" "False"
ok "  origin's push URL is back to normal" "$(git -C "$SB/work" remote get-url --push origin)" "remote.git"
ok "  the hook is gone" "$([ -e "$SB/work/.git/hooks/pre-push" ] && echo present || echo gone)" "gone"
ok "  a terminal push works again" "$(git -C "$SB/work" push origin main 2>&1; echo rc=$?)" "rc=0"

section "An existing pre-push hook (Git LFS, as in argos) is preserved"
cat > "$SB/work/.git/hooks/pre-push" <<'HOOK'
#!/bin/sh
echo "LFS-STYLE HOOK RAN" >> "$(git rev-parse --git-dir)/lfs-ran.log"
exit 0
HOOK
chmod +x "$SB/work/.git/hooks/pre-push"
probe block "$SB/work" "on" >/dev/null
ok "the foreign hook was saved, not discarded" "$([ -e "$SB/work/.git/hooks/pre-push.gitswitch-saved" ] && echo saved || echo LOST)" "saved"
ok "  and ours chains it" "$(cat "$SB/work/.git/hooks/pre-push")" "pre-push.gitswitch-saved"
probe block "$SB/work" "off" >/dev/null
ok "unblocking puts the original hook back" "$(cat "$SB/work/.git/hooks/pre-push")" "LFS-STYLE HOOK RAN"
ok "  and removes the saved copy" "$([ -e "$SB/work/.git/hooks/pre-push.gitswitch-saved" ] && echo present || echo gone)" "gone"
cat > "$SB/work/.git/hooks/pre-push" <<'HOOK'
#!/bin/sh
echo "ran" >> "$(git rev-parse --git-dir)/lfs-ran.log"
exit 0
HOOK
chmod +x "$SB/work/.git/hooks/pre-push"
probe block "$SB/work" "on" >/dev/null
git -C "$SB/work" config --local --unset-all 'url.gitswitch-push-blocked://.pushInsteadOf' 2>/dev/null
git -C "$SB/work" config --local --unset gitswitch.pushBlocked 2>/dev/null
printf 'p3\n' >> "$SB/work/a.txt"; git -C "$SB/work" add -A >/dev/null; git -C "$SB/work" commit -qm "chain test"
git -C "$SB/work" push -q origin main 2>/dev/null
ok "with the flag off the chained hook still runs (guard is inert)" "$(cat "$SB/work/.git/lfs-ran.log" 2>/dev/null)" "ran"
probe block "$SB/work" "off" >/dev/null

section "Pull: fast-forward"
git clone -q "$SB/remote.git" "$SB/other" 2>/dev/null
cd "$SB/other"; git config user.email "other@example.test"; git config user.name "Other"
printf 'from other\n' >> a.txt; git add -A >/dev/null; git commit -qm "remote work"; git push -q origin main 2>/dev/null
cd "$SB/work"
git -C "$SB/work" fetch -q origin 2>/dev/null
OUT=$(probe pull "$SB/work" "ff-only")
ok "fast-forwards when only the remote moved" "$(printf '%s' "$OUT" | jqf headline)" "Pulled 1 commit(s) with fast-forward only"
ok "  and says it was a fast-forward" "$(printf '%s' "$OUT" | jqf pull.fast_forward)" "True"
ok "  with a recovery command" "$(printf '%s' "$OUT" | jqf pull.recovery)" "git reset --hard"

section "Pull: diverged"
cd "$SB/other"; printf 'theirs\n' >> b.txt; git add -A >/dev/null; git commit -qm "their commit"; git push -q origin main 2>/dev/null
cd "$SB/work"; printf 'mine\n' >> c.txt; git add -A >/dev/null; git commit -qm "my commit"
OUT=$(probe pull "$SB/work" "ff-only")
ok "fast-forward refuses on a diverged branch" "$(printf '%s' "$OUT" | jqf advice.action)" "choose-pull-mode"
ok "  and explains both remaining choices" "$(printf '%s' "$OUT" | jqf advice.guidance)" "Rebase"
ok "  nothing moved" "$(git -C "$SB/work" log -1 --format=%s)" "my commit"
OUT=$(probe pull "$SB/work" "rebase")
ok "rebase replays local work on top, counting only the upstream commit (not the replayed one)" "$(printf '%s' "$OUT" | jqf headline)" "Pulled 1 commit(s) with rebase."
ok "  history is now linear" "$(git -C "$SB/work" log --oneline -3 | head -1 | sed 's/^[a-f0-9]* //')" "my commit"
ok "  and the rebase was a fast-forward over their commit" "$(git -C "$SB/work" log --format=%s -3 | tr '\n' '|')" "my commit|their commit"

section "Pull: rebase refuses on a dirty tree"
printf 'dirty\n' >> "$SB/work/a.txt"
OUT=$(probe pull "$SB/work" "rebase")
ok "refuses rather than silently autostashing" "$(printf '%s' "$OUT" | jqf refusal.code)" "dirty-tree"
ok "  ff-only is still allowed with a dirty tree" "$(probe pull "$SB/work" "ff-only" | jqf refusal.code)" "None"
git -C "$SB/work" checkout -q -- a.txt

section "Pull: rebase with autostash on a dirty tree"
cd "$SB/other"; git pull -q --rebase origin main 2>/dev/null; printf 'd\n' > d.txt; git add -A >/dev/null; git commit -qm "their d"; git push -q origin main 2>/dev/null
printf 'dirty\n' >> "$SB/work/a.txt"
OUT=$(probe pull "$SB/work" "rebase,autostash")
ok "autostash lets the rebase run" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  the dirty edit is back" "$(git -C "$SB/work" status --porcelain -- a.txt)" " M a.txt"
ok "  no stash was left behind" "$(git -C "$SB/work" stash list | wc -l | tr -d ' ')" "0"
ok "  upstream's commit arrived" "$([ -e "$SB/work/d.txt" ] && echo present || echo missing)" "present"
ok "  and the result does not claim a conflict" "$(printf '%s' "$OUT" | jqf pull.autostash_conflict)" "False"
git -C "$SB/work" checkout -q -- a.txt
cd "$SB/other"; printf 'theirs edit\n' >> a.txt; git add -A >/dev/null; git commit -qm "their a"; git push -q origin main 2>/dev/null
printf 'my edit\n' >> "$SB/work/a.txt"
OUT=$(probe pull "$SB/work" "rebase,autostash")
ok "a stash that cannot be re-applied is reported, not hidden" "$(printf '%s' "$OUT" | jqf pull.autostash_conflict)" "True"
ok "  the rebase itself finished" "$(printf '%s' "$OUT" | jqf operation)" "None"
ok "  the file is conflicted" "$(printf '%s' "$OUT" | jqf conflicted)" "1"
ok "  the status names the autostash as the source" "$(printf '%s' "$OUT" | jqf conflict_source)" "autostash"
ok "  the stash entry is kept" "$(git -C "$SB/work" stash list | grep -c autostash)" "1"
ok "  and the advice says the work is safe in the stash" "$(printf '%s' "$OUT" | jqf advice.guidance)" "stash@{0}"
OUT=$(probe discard "$SB/work" "a.txt")
ok "discarding the conflicted file is allowed when no operation is in progress" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  leaving a clean tree" "$(printf '%s' "$OUT" | jqf conflicted)" "0"
ok "  with the stash still there to pop by hand" "$(git -C "$SB/work" stash list | grep -c autostash)" "1"
# That leftover must not poison the next pull: with it still in the list, a
# clean autostash pull is reported as clean.
cd "$SB/other"; printf 'e\n' > e.txt; git add -A >/dev/null; git commit -qm "their e"; git push -q origin main 2>/dev/null
cd "$SB/work"; printf 'edit elsewhere\n' >> "$SB/work/keep.txt"
OUT=$(probe pull "$SB/work" "rebase,autostash")
ok "an old 'autostash' entry does not make a clean autostash pull look failed" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  autostash_conflict is false" "$(printf '%s' "$OUT" | jqf pull.autostash_conflict)" "False"
ok "  the headline counts the one upstream commit" "$(printf '%s' "$OUT" | jqf headline)" "Pulled 1 commit(s) with rebase."
ok "  the edit is back in the tree" "$(git -C "$SB/work" status --porcelain -- keep.txt)" " M keep.txt"
ok "  the old entry is untouched" "$(git -C "$SB/work" stash list --format=%gs | tr '\n' '|')" "autostash|"
ok "  and their commit arrived" "$([ -e "$SB/work/e.txt" ] && echo present || echo missing)" "present"
git -C "$SB/work" checkout -q -- keep.txt
git -C "$SB/work" stash drop -q

section "Pull: merge and conflicts"
git -C "$SB/work" push -q origin main 2>/dev/null
cd "$SB/other"; git pull -q --rebase origin main 2>/dev/null; printf 'THEIRS\n' > conflict.txt; git add -A >/dev/null; git commit -qm "their conflict"; git push -q origin main 2>/dev/null
cd "$SB/work"; printf 'MINE\n' > conflict.txt; git add -A >/dev/null; git commit -qm "my conflict"
OUT=$(probe pull "$SB/work" "merge")
ok "a merge conflict is reported, not resolved" "$(printf '%s' "$OUT" | jqf advice.action)" "resolve-conflicts"
ok "  it never claims to have fixed anything" "$(printf '%s' "$OUT" | jqf advice.guidance)" "Nothing was resolved automatically"
ok "  the conflict is left in place for the user" "$(printf '%s' "$OUT" | jqf conflicted)" "1"
OUT=$(probe status "$SB/work")
ok "  status shows the merge in progress" "$(printf '%s' "$OUT" | jqf operation.kind)" "merge"
ok "  with the exact abort command" "$(printf '%s' "$OUT" | jqf operation.abort_command)" "git merge --abort"
ok "  and names the conflict in plain words" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([e['conflict'] for e in d['entries'] if e['kind']=='conflicted'])")" "both added"
OUT=$(probe commit "$SB/work" "cannot commit with conflicts")
ok "commit is refused while conflicts remain" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-paths"
OUT=$(probe unstage "$SB/work" "conflict.txt")
ok "unstaging a conflicted file is refused (it would destroy the merge state)" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-paths"
OUT=$(probe abort "$SB/work")
ok "abort puts the branch back" "$(printf '%s' "$OUT" | jqf headline)" "Aborted the merge"
ok "  my commit is still there" "$(git -C "$SB/work" log -1 --format=%s)" "my conflict"
ok "  and nothing is conflicted now" "$(printf '%s' "$OUT" | jqf conflicted)" "0"

section "Merge that succeeds can be concluded by one commit"
printf 'MINE\n' > "$SB/work/conflict.txt"; git -C "$SB/work" add -A >/dev/null; git -C "$SB/work" commit -qm "keep mine" >/dev/null 2>&1
git -C "$SB/work" merge -q --no-commit --no-ff origin/main 2>/dev/null
cd "$SB/work"; git checkout -q --ours conflict.txt 2>/dev/null; git add conflict.txt 2>/dev/null
OUT=$(probe commit "$SB/work" "finish the merge")
ok "a commit during a merge is allowed, not blocked" "$(printf '%s' "$OUT" | jqf ok)$(printf '%s' "$OUT" | jqf refusal.code)" "True"

section "Continue finishes a stopped rebase, skipping a pick that became empty"
git -C "$SB/work" push -q origin main 2>/dev/null
cd "$SB/other"; git pull -q --rebase origin main 2>/dev/null; printf 'THEIRS-2\n' > conflict.txt; git add -A >/dev/null; git commit -qm "their second"; git push -q origin main 2>/dev/null
cd "$SB/work"; printf 'MINE-2\n' > conflict.txt; git add -A >/dev/null; git commit -qm "my second"
OUT=$(probe pull "$SB/work" "rebase")
ok "the rebase stops on the conflict" "$(printf '%s' "$OUT" | jqf advice.action)" "resolve-conflicts"
OUT=$(probe status "$SB/work")
ok "  status names the continue command" "$(printf '%s' "$OUT" | jqf operation.continue_command)" "git rebase --continue"
ok "  and the side coming in by its upstream name, not the id git stores" "[$(printf '%s' "$OUT" | jqf operation.sides.theirs)]" "[origin/main]"
ok "  in the banner too" "$(printf '%s' "$OUT" | jqf operation.detail)" "main onto origin/main"
OUT=$(probe continue "$SB/work")
ok "continue is refused while the file is conflicted" "$(printf '%s' "$OUT" | jqf refusal.code)" "unmerged-paths"
printf 'BOTH\n' > "$SB/work/conflict.txt"; git -C "$SB/work" add conflict.txt
OUT=$(probe continue "$SB/work")
ok "after resolving and staging, continue finishes the rebase" "$(printf '%s' "$OUT" | jqf headline)" "Finished the rebase"
ok "  my commit is on top" "$(git -C "$SB/work" log -1 --format=%s)" "my second"
ok "  and no operation is left" "$(printf '%s' "$OUT" | jqf operation)" "None"
# A resolution identical to upstream's side leaves nothing to commit: git
# refuses --continue for that pick, so GitSwitch skips it and says so.
git -C "$SB/work" push -q origin main 2>/dev/null
cd "$SB/other"; git pull -q --rebase origin main 2>/dev/null; printf 'THEIRS-3\n' > conflict.txt; git add -A >/dev/null; git commit -qm "their third"; git push -q origin main 2>/dev/null
cd "$SB/work"; printf 'MINE-3\n' > conflict.txt; git add -A >/dev/null; git commit -qm "my third"
OUT=$(probe pull "$SB/work" "rebase")
printf 'THEIRS-3\n' > "$SB/work/conflict.txt"; git -C "$SB/work" add conflict.txt
OUT=$(probe continue "$SB/work")
ok "a pick that became empty is skipped, not stuck" "$(printf '%s' "$OUT" | jqf detail)" "skipped"
ok "  the rebase is finished" "$(printf '%s' "$OUT" | jqf headline)" "Finished the rebase"
ok "  and HEAD is upstream's commit" "$(git -C "$SB/work" log -1 --format=%s)" "their third"
OUT=$(probe continue "$SB/work")
ok "with nothing in progress, continue is refused by name" "$(printf '%s' "$OUT" | jqf refusal.code)" "nothing-to-continue"

section "Diff viewer"
printf 'diffline\n' >> "$SB/work/a.txt"
OUT=$(probe diff "$SB/work" "a.txt,unstaged")
ok "renders an unstaged text diff" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([l['kind'] for l in d['lines']])")" "add"
ok "  and counts the added lines" "$(printf '%s' "$OUT" | jqf added)" "1"
printf '\x00\x01\x02binary\x00' > "$SB/work/blob.bin"
git -C "$SB/work" add blob.bin >/dev/null
OUT=$(probe diff "$SB/work" "blob.bin,staged")
ok "detects a binary file instead of rendering it" "$(printf '%s' "$OUT" | jqf is_binary)" "True"
ok "  and says so" "$(printf '%s' "$OUT" | jqf empty_reason)" "Binary file"
python3 -c "
open('$SB/work/big.txt','w').write('\n'.join('line %d'%i for i in range(8000)))"
git -C "$SB/work" add big.txt >/dev/null
OUT=$(probe diff "$SB/work" "big.txt,staged")
ok "caps a huge diff instead of shipping it all" "$(printf '%s' "$OUT" | jqf truncated)" "True"
git -C "$SB/work" reset -q

section "Who would push: the account behind the remote, before anything is sent"
# A fake ssh stands in for github.com: it greets with the key it was handed, or
# "agent-user" with none. -G and -F go to the real ssh, so alias resolution is
# the real thing. GITSWITCH_SSH_CONFIG (debug builds) points ssh at a sandbox
# config, because ssh finds ~ through passwd, not $HOME.
FAKE="$SB/fakebin"; mkdir -p "$FAKE" "$SB/keys"; REAL_SSH="$(command -v ssh)"
printf 'not a key\n' > "$SB/keys/work_key"; printf 'not a key\n' > "$SB/keys/alias_key"
cat > "$FAKE/ssh" <<FAKESSH
#!/bin/sh
case "\$1" in -G|-F) exec "$REAL_SSH" "\$@" ;; esac
# As ssh does: an explicit -i wins; otherwise the agent's key is offered first
# unless the Host block says IdentitiesOnly, in which case its IdentityFile is.
key=""; port=""; prev=""; dest=""
for a in "\$@"; do
  [ "\$prev" = "-i" ] && key="\$a"; [ "\$prev" = "-p" ] && port="\$a"; prev="\$a"; dest="\$a"
done
host="\${dest#*@}"
if [ -z "\$key" ]; then
  cfg="\$("$REAL_SSH" -F "\$GITSWITCH_SSH_CONFIG" -G "\$host" 2>/dev/null)"
  if printf '%s\n' "\$cfg" | grep -q '^identitiesonly yes'; then key="\$(printf '%s\n' "\$cfg" | awk '/^identityfile /{print \$2; exit}')"; fi
fi
if [ -n "\$key" ]; then who="\$(basename "\$key")"; elif [ -n "\$port" ]; then who="port-\$port"; else who="agent-user"; fi
echo "Hi \$who! You've successfully authenticated, but GitHub does not provide shell access." >&2
exit 1
FAKESSH
chmod +x "$FAKE/ssh"
# The alias names a key; every other host names one that does not exist, so
# this machine's own ~/.ssh keys never leak into the answers.
printf 'Host gh-work\n  HostName github.com\n  User git\n  IdentityFile %s\n  IdentitiesOnly yes\nHost *\n  IdentityFile %s\n' "$SB/keys/alias_key" "$SB/keys/none" > "$SB/sshcfg"
WJ="$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1],"refresh":False}))' "$SB/work")"
who() { PATH="$FAKE:$PATH" GITSWITCH_SSH_CONFIG="$SB/sshcfg" GITSWITCH_GITHUB_API="http://127.0.0.1:9" PROBE_OP=invoke PROBE_CMD=changes_push_identity PROBE_JSON="$WJ" "$BIN" probe --ignored --nocapture 2>/dev/null | sed -n 's/^PROBE_OUT //p'; }
ok "a local-path remote involves no account" "$(who | jqf value.via)" "local"
git -C "$SB/work" remote set-url --push origin git@github.com:acme/thing.git
git -C "$SB/work" config core.sshCommand "ssh -i $SB/keys/work_key -o IdentitiesOnly=yes"
OUT=$(who)
ok "with a profile key (core.sshCommand) the push uses that key" "$(printf '%s' "$OUT" | jqf value.via):$(printf '%s' "$OUT" | jqf value.key_path)" "ssh-key:$SB/keys/work_key"
ok "  and github.com's greeting names the account" "$(printf '%s' "$OUT" | jqf value.login)" "work_key"
ok "  the push URL is reported" "$(printf '%s' "$OUT" | jqf value.url)" "git@github.com:acme/thing.git"
git -C "$SB/work" config --unset core.sshCommand
git -C "$SB/work" remote set-url --push origin git@gh-work:acme/thing.git
OUT=$(who)
ok "an ssh alias is resolved through ssh -G to its real host" "$(printf '%s' "$OUT" | jqf value.host)" "github.com"
ok "  and, with IdentitiesOnly, to the key the Host block pins" "$(printf '%s' "$OUT" | jqf value.key_path)" "$SB/keys/alias_key"
ok "  which is the account that would push" "$(printf '%s' "$OUT" | jqf value.login)" "alias_key"
git -C "$SB/work" remote set-url --push origin ssh://git@ssh.github.com:443/acme/thing.git
OUT=$(who)
ok "the firewall form ssh://…:443 is probed on its own port" "$(printf '%s' "$OUT" | jqf value.host):$(printf '%s' "$OUT" | jqf value.login)" "ssh.github.com:port-443"
git -C "$SB/work" remote set-url --push origin git@github.com:acme/thing.git
OUT=$(who)
ok "with no key configured anywhere, the agent decides" "$(printf '%s' "$OUT" | jqf value.via):$(printf '%s' "$OUT" | jqf value.login)" "ssh-agent:agent-user"
git -C "$SB/work" remote set-url --push origin git@gitlab.example.com:acme/thing.git
OUT=$(who)
ok "a host that isn't GitHub is named, not guessed at" "$(printf '%s' "$OUT" | jqf value.login):$(printf '%s' "$OUT" | jqf value.detail)" "None:gitlab.example.com isn't GitHub"
git -C "$SB/work" remote set-url --push origin https://github.com/acme/thing.git
OUT=$(who)
ok "an HTTPS remote with no stored credential says the push would fail" "$(printf '%s' "$OUT" | jqf value.via):$(printf '%s' "$OUT" | jqf value.login)" "https-credential:None"
ok "  and why" "$(printf '%s' "$OUT" | jqf value.detail)" "No credential is stored"
git -C "$SB/work" config credential.helper '!f() { echo username=hthelper; echo password=not-a-real-secret; }; f'
OUT=$(who)
# For github.com the account comes from GitHub's answer to the token, never from
# the stored user name (a name is whatever was typed when it was saved). The
# API here is a closed port, so the honest answer is "could not confirm".
ok "a stored GitHub credential is named, but not trusted as the account" "$(printf '%s' "$OUT" | jqf value.account):$(printf '%s' "$OUT" | jqf value.login)" "hthelper:None"
ok "  and says GitHub could not be asked" "$(printf '%s' "$OUT" | jqf value.detail)" "could not be reached"
ok "  without the secret anywhere in the answer" "$(printf '%s' "$OUT" | grep -c "not-a-real-secret" || true)" "0"
git -C "$SB/work" config credential.helper '!f() { echo username=ghp_000000000000000000000000000000000000; echo password=x-oauth-basic; }; f'
OUT=$(who)
ok "a token travelling as the user name is treated as the secret, not shown" "$(printf '%s' "$OUT" | grep -c "ghp_0000" || true)" "0"
git -C "$SB/work" remote set-url --push origin https://gitlab.example.com/acme/thing.git
git -C "$SB/work" config credential.helper '!f() { echo username=hthelper; echo password=not-a-real-secret; }; f'
OUT=$(who)
ok "for a host that is not GitHub, the stored user name is the best available answer" "$(printf '%s' "$OUT" | jqf value.login)" "hthelper"
git -C "$SB/work" config --unset credential.helper
# A block rewrites the remote's URL, never an explicit pushurl (git's rule, and
# the guard's documented gap that the hook covers), so the URL itself moves here.
git -C "$SB/work" config --unset remote.origin.pushurl
git -C "$SB/work" remote set-url origin https://github.com/acme/thing.git
probe block "$SB/work" "on" >/dev/null
ok "a blocked repository says so instead of naming an account" "$(who | jqf value.via)" "blocked"
probe block "$SB/work" "off" >/dev/null
git -C "$SB/work" remote set-url origin "$SB/remote.git"

section "A repository behind an SSH alias for GitHub has its size asked like github.com's"
# A fake API that logs every request and says each acme repository weighs
# 8 MiB, and a gh that lists no account: no sign-in of this machine is used.
printf 'Host gl-work\n  HostName gitlab.example.com\n  User git\n' >> "$SB/sshcfg"
printf '#!/bin/sh\nexit 1\n' > "$FAKE/gh"; chmod +x "$FAKE/gh"
"$PY" - "$SB/size-api.log" > "$SB/size-api-port" <<'PYAPI' &
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
SIZE_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/size-api-port" ] && break; sleep 0.5; done
SIZE_API="http://127.0.0.1:$(cat "$SB/size-api-port")"
size_of() {
  local j; j="$("$PY" -c 'import json,sys;print(json.dumps({"url":sys.argv[1]}))' "$1")"
  PATH="$FAKE:$PATH" GITSWITCH_SSH_CONFIG="$SB/sshcfg" GITSWITCH_GITHUB_API="$SIZE_API" pinvoke clone_size_estimate "$j" \
    | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print("none" if v is None else "%s %s" % (v["repo"], v["kb"]))'
}
ok "an SSH alias for github.com is asked about like github.com" "$(size_of git@gh-work:acme/thing.git)" "acme/thing 8192"
ok "  ssh.github.com on port 443 too" "$(size_of ssh://git@ssh.github.com:443/acme/other.git)" "acme/other 8192"
ok "  anonymously, with no account signed in here" "$(grep -c 'token=yes' "$SB/size-api.log")" "0"
: > "$SB/size-api.log"
ok "an alias for another host is not asked about" "$(size_of git@gl-work:acme/third.git):$(wc -l < "$SB/size-api.log" | tr -d ' ')" "none:0"
ok "  nor is an address that is no host at all" "$(size_of "$SB/remote.git"):$(wc -l < "$SB/size-api.log" | tr -d ' ')" "none:0"
{ kill "$SIZE_PID" && wait "$SIZE_PID"; } 2>/dev/null

section "Submodules"
git init -q --bare "$SB/sub.git"
git clone -q "$SB/sub.git" "$SB/subwork" 2>/dev/null
cd "$SB/subwork"; git config user.email s@e.test; echo "sub" > s.txt; git add -A >/dev/null; git commit -qm "sub init"; git push -q origin main 2>/dev/null
cd "$SB/work"
git -c protocol.file.allow=always submodule add -q "$SB/sub.git" vendor/sub 2>/dev/null
git add -A >/dev/null; git commit -qm "add submodule" >/dev/null
rm -rf "$SB/work/vendor/sub"; mkdir -p "$SB/work/vendor/sub"
OUT=$(probe submodule "$SB/work")
ok "submodule update reports what it restored" "$(printf '%s' "$OUT" | jqf headline)" "submodule(s) up to date"
ok "  and the submodule really has its files back" "$([ -e "$SB/work/vendor/sub/s.txt" ] && echo present || echo MISSING)" "present"

section "A pull or a push is stopped for silence, never for taking long"
# The transfer used to have three minutes in total: a big repository on an
# ordinary connection was cut off while it was still arriving. Now it runs
# with git's progress on, and only a silence that long stops it.
#
# The remote speaks the pack protocol (file://), and the suite's own config
# sends every pack through a hook that trickles it out — a slow connection.
git init -q --bare "$SB/net.git"; git clone -q "file://$SB/net.git" "$SB/net-author" 2>/dev/null
printf 'base\n' > "$SB/net-author/base.txt"; git -C "$SB/net-author" add -A >/dev/null; git -C "$SB/net-author" commit -qm base; git -C "$SB/net-author" push -q origin main 2>/dev/null
git clone -q "file://$SB/net.git" "$SB/net-work" 2>/dev/null
git clone -q "file://$SB/net.git" "$SB/net-stuck" 2>/dev/null
git clone -q "file://$SB/net.git" "$SB/net-history" 2>/dev/null
# A repository whose own housekeeping becomes due with the pull's fetch (its
# second pack), cannot detach (as on Windows) and is slow: its pre-auto-gc
# hook sleeps longer than the silence window. Cloned before upstream moves.
git clone -q "file://$SB/net.git" "$SB/net-gc" 2>/dev/null
git -C "$SB/net-gc" config gc.autoDetach false; git -C "$SB/net-gc" config gc.autoPackLimit 1
GC_HOOK="$(git -C "$SB/net-gc" rev-parse --path-format=absolute --git-path hooks)/pre-auto-gc"
printf '#!/bin/sh\necho ran >> "%s"\nsleep 6\n' "$SB/gc-hook.log" > "$GC_HOOK"; chmod +x "$GC_HOOK"
# A second remote, as a fork and its original are: with two remotes `fetch
# --all` hands each to a child fetch that reports nothing into a pipe.
git init -q --bare "$SB/net2.git"; git clone -q "file://$SB/net2.git" "$SB/net2-author" 2>/dev/null
printf 'other\n' > "$SB/net2-author/other.txt"; git -C "$SB/net2-author" add -A >/dev/null; git -C "$SB/net2-author" commit -qm other; git -C "$SB/net2-author" push -q origin main 2>/dev/null
git -C "$SB/net-history" remote add second "file://$SB/net2.git"; git -C "$SB/net-history" fetch -q second 2>/dev/null
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24; do head -c 131072 /dev/urandom > "$SB/net-author/blob$i.bin"; head -c 131072 /dev/urandom > "$SB/net2-author/more$i.bin"; done
git -C "$SB/net-author" add -A >/dev/null; git -C "$SB/net-author" commit -qm "about 3 MiB that will not compress"; git -C "$SB/net-author" push -q origin main 2>/dev/null
git -C "$SB/net2-author" add -A >/dev/null; git -C "$SB/net2-author" commit -qm "about 3 MiB more, on the second remote"; git -C "$SB/net2-author" push -q origin main 2>/dev/null
cat > "$SB/trickle.sh" <<'TRICKLE'
#!/bin/sh
# Run the real program and pass its output on a little at a time.
"$@" | perl -e '$|=1; binmode STDIN; binmode STDOUT; while (read(STDIN, $b, 32768)) { print $b; select(undef, undef, undef, 0.1); }'
TRICKLE
chmod +x "$SB/trickle.sh"
git config --global uploadpack.packObjectsHook "$SB/trickle.sh"
T0=$(date +%s); OUT=$(GITSWITCH_STALL_SECS=3 probe pull "$SB/net-work" "ff-only"); T1=$(date +%s)
# The History page's Fetch goes down the same slow road, from both remotes.
H0=$(date +%s); HOUT=$(GITSWITCH_STALL_SECS=3 probe history_fetch "$SB/net-history"); H1=$(date +%s)
git config --global --unset uploadpack.packObjectsHook
G0=$(date +%s); GOUT=$(GITSWITCH_STALL_SECS=3 probe pull "$SB/net-gc" "ff-only"); G1=$(date +%s)
ok "a pull that takes longer than the silence allowed, but keeps arriving, is not stopped" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$SB/net-work" rev-parse HEAD)" "True:$(git -C "$SB/net-author" rev-parse HEAD)"
ok "  it really was slower than the window (3 s)" "$([ $((T1-T0)) -ge 5 ] && echo "slow enough" || echo "too quick to prove anything: $((T1-T0))s")" "slow enough"
ok "  and every file arrived" "$(ls "$SB/net-work" | grep -c '^blob')" "24"
ok "  git's progress counters stay out of the result" "$(printf '%s' "$OUT" | grep -c 'Receiving objects')" "0"
ok "History's Fetch, just as slow, is not stopped either: the remote branch moved" "$(printf '%s' "$HOUT" | "$PY" -c 'import sys,json;print("moved" if json.load(sys.stdin).get("updated_remote_refs",0)>=1 else "none")'):$(git -C "$SB/net-history" rev-parse origin/main)" "moved:$(git -C "$SB/net-author" rev-parse HEAD)"
ok "  and from the second remote as well — two remotes are fetched one by one, each with its progress" "$(git -C "$SB/net-history" rev-parse second/main)" "$(git -C "$SB/net2-author" rev-parse HEAD)"
ok "  it really was slower than the window (3 s)" "$([ $((H1-H0)) -ge 5 ] && echo "slow enough" || echo "too quick to prove anything: $((H1-H0))s")" "slow enough"
ok "a pull is not stopped by the repository's own housekeeping, slow and in the foreground: it runs after the fetch, not inside it" "$(printf '%s' "$GOUT" | jqf ok):$(git -C "$SB/net-gc" rev-parse HEAD)" "True:$(git -C "$SB/net-author" rev-parse HEAD)"
ok "  and it did run — git's housekeeping still happens" "$([ -s "$SB/gc-hook.log" ] && echo ran || echo never)" "ran"
ok "  it really was slower than the window (3 s)" "$([ $((G1-G0)) -ge 5 ] && echo "slow enough" || echo "too quick to prove anything: $((G1-G0))s")" "slow enough"
ok "  and it fetched without touching the branch, the files or any local branch" "$(printf '%s' "$HOUT" | jqf head_unchanged):$(printf '%s' "$HOUT" | jqf worktree_unchanged):$(printf '%s' "$HOUT" | jqf local_branches_unchanged):$(git -C "$SB/net-history" rev-parse HEAD)" "True:True:True:$(git -C "$SB/net-author" rev-parse HEAD~1)"

# A server that accepts the connection and never answers.
"$PY" - > "$SB/stuck-port" <<'PYSRV' &
import socket, time
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(8)
print(s.getsockname()[1], flush=True)
while True:
    c, _ = s.accept(); time.sleep(3600)
PYSRV
STUCK_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/stuck-port" ] && break; sleep 0.5; done
PORT="$(cat "$SB/stuck-port")"
git -C "$SB/net-stuck" remote set-url origin "git://127.0.0.1:$PORT/net.git"
T0=$(date +%s); OUT=$(GITSWITCH_STALL_SECS=3 probe pull "$SB/net-stuck" "ff-only"); T1=$(date +%s)
ok "a pull from a server that never answers is stopped" "$OUT" "made no progress for 3s and was stopped"
ok "  after the silence, not after three minutes" "$([ $((T1-T0)) -lt 60 ] && echo quick || echo "slow: $((T1-T0))s")" "quick"
ok "  and nothing moved" "$(git -C "$SB/net-stuck" rev-parse HEAD):$(git -C "$SB/net-stuck" status --porcelain | wc -l | tr -d ' ')" "$(git -C "$SB/net-author" rev-parse HEAD~1):0"
T0=$(date +%s); OUT=$(GITSWITCH_STALL_SECS=3 probe history_fetch "$SB/net-stuck"); T1=$(date +%s)
ok "History's Fetch from a server that never answers is stopped too — it used to wait for ever" "$OUT" "made no progress for 3s and was stopped"
ok "  after the silence" "$([ $((T1-T0)) -lt 60 ] && echo quick || echo "slow: $((T1-T0))s")" "quick"
printf 'mine\n' > "$SB/net-stuck/mine.txt"; git -C "$SB/net-stuck" add -A >/dev/null; git -C "$SB/net-stuck" commit -qm "mine"
T0=$(date +%s); OUT=$(GITSWITCH_STALL_SECS=3 probe push "$SB/net-stuck" ""); T1=$(date +%s)
ok "a push to a server that never answers is stopped the same way" "$OUT" "made no progress for 3s and was stopped"
ok "  after the silence, not after three minutes" "$([ $((T1-T0)) -lt 60 ] && echo quick || echo "slow: $((T1-T0))s")" "quick"
ok "  and the commit is still here, unpushed" "$(git -C "$SB/net-stuck" log -1 --format=%s)" "mine"
{ kill "$STUCK_PID" && wait "$STUCK_PID"; } 2>/dev/null

# A slow push: the far side reads the pack a little at a time.
cat > "$SB/slow-receive.sh" <<'SLOWRCV'
#!/bin/sh
# What `git push` runs on the far side, with its input slowed down.
perl -e '$|=1; binmode STDIN; binmode STDOUT; while (read(STDIN, $b, 32768)) { print $b; select(undef, undef, undef, 0.1); }' | git receive-pack "$@"
SLOWRCV
chmod +x "$SB/slow-receive.sh"
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24; do head -c 131072 /dev/urandom > "$SB/net-work/mine$i.bin"; done
git -C "$SB/net-work" add -A >/dev/null; git -C "$SB/net-work" commit -qm "about 3 MiB of mine"
git -C "$SB/net-work" config remote.origin.receivepack "$SB/slow-receive.sh"
T0=$(date +%s); OUT=$(GITSWITCH_STALL_SECS=3 probe push "$SB/net-work" ""); T1=$(date +%s)
git -C "$SB/net-work" config --unset remote.origin.receivepack
ok "a push that takes longer than the silence allowed, but keeps going, is not stopped" "$(printf '%s' "$OUT" | jqf ok):$(git -C "$SB/net.git" rev-parse main)" "True:$(git -C "$SB/net-work" rev-parse HEAD)"
ok "  it really was slower than the window (3 s)" "$([ $((T1-T0)) -ge 5 ] && echo "slow enough" || echo "too quick to prove anything: $((T1-T0))s")" "slow enough"

verify_result
