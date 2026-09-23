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
ok "rebase replays local work on top" "$(printf '%s' "$OUT" | jqf headline)" "Pulled"
ok "  history is now linear" "$(git -C "$SB/work" log --oneline -3 | head -1 | sed 's/^[a-f0-9]* //')" "my commit"
ok "  and the rebase was a fast-forward over their commit" "$(git -C "$SB/work" log --format=%s -3 | tr '\n' '|')" "my commit|their commit"

section "Pull: rebase refuses on a dirty tree"
printf 'dirty\n' >> "$SB/work/a.txt"
OUT=$(probe pull "$SB/work" "rebase")
ok "refuses rather than silently autostashing" "$(printf '%s' "$OUT" | jqf refusal.code)" "dirty-tree"
ok "  ff-only is still allowed with a dirty tree" "$(probe pull "$SB/work" "ff-only" | jqf refusal.code)" "None"
git -C "$SB/work" checkout -q -- a.txt

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

verify_result
