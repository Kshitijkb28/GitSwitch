#!/bin/bash
# Regression suite: the features that existed BEFORE the Changes page — clone,
# submodule clone, sparse clone, the commit identity guard, and History.
# Throwaway repos, isolated HOME, no network, no accounts.
set -u
source "$(dirname "$0")/lib.sh"
verify_init regress "Regress User" "regress@example.test"

# An "upstream" repo with history, a submodule and a couple of branches.
git init -q --bare "$SB/sub.git"
git clone -q "$SB/sub.git" "$SB/subwork" 2>/dev/null
cd "$SB/subwork"; echo sub > s.txt; git add -A >/dev/null; git commit -qm "sub init"; git push -q origin main 2>/dev/null

git init -q --bare "$SB/origin.git"
git clone -q "$SB/origin.git" "$SB/seed" 2>/dev/null
cd "$SB/seed"
mkdir -p docs src
echo one > src/a.txt; echo doc > docs/readme.md
git add -A >/dev/null; git commit -qm "first"
echo two >> src/a.txt; git add -A >/dev/null; git commit -qm "second"
git submodule add -q "$SB/sub.git" vendor/sub 2>/dev/null
git add -A >/dev/null; git commit -qm "add submodule"
git checkout -q -b feature; echo f > src/f.txt; git add -A >/dev/null; git commit -qm "feature work"
git checkout -q main
git push -q origin main 2>/dev/null; git push -q origin feature 2>/dev/null

mkdir -p "$SB/clones"   # the clone destination parent must exist

section "Clone (full)"
OUT=$(probe clone "" "$SB/origin.git,$SB/clones,plain")
ok "clones into the named folder" "$(printf '%s' "$OUT" | jqf path)" "$SB/clones/plain"
ok "  and the files are really there" "$([ -f "$SB/clones/plain/src/a.txt" ] && echo yes || echo no)" "yes"
ok "  with full history" "$(git -C "$SB/clones/plain" rev-list --count HEAD)" "3"

section "Clone with submodules"
OUT=$(probe clone "" "$SB/origin.git,$SB/clones,withsub,submodules")
ok "reports the submodule count" "$(printf '%s' "$OUT" | jqf submodules.listed)" "1"
ok "  and checks it out (not left empty)" "$([ -f "$SB/clones/withsub/vendor/sub/s.txt" ] && echo yes || echo no)" "yes"
ok "  sets submodule.active so future ones arrive on pull" "$(git -C "$SB/clones/withsub" config --get submodule.active)" "."

section "Sparse clone"
OUT=$(probe sparse_clone "" "$SB/origin.git,$SB/clones,sparse")
ok "sparse clone succeeds" "$(printf '%s' "$OUT" | jqf path)" "$SB/clones/sparse"
OUT=$(probe sparse_info "$SB/clones/sparse")
ok "  lists the repo's top-level folders" "$(printf '%s' "$OUT" | jqf available_dirs)" "docs"
ok "  and reports the branch" "$(printf '%s' "$OUT" | jqf branch)" "main"

section "Clone refusals are explained, not raw"
OUT=$(probe clone "" "$SB/does-not-exist.git,$SB/clones,nope")
ok "a bad URL fails with git's own words kept" "$OUT" "error"
ok "  and does not leave a half-written folder" "$([ -d "$SB/clones/nope" ] && echo left || echo clean)" "clean"

section "Commit identity guard (install / chain / uninstall)"
R="$SB/clones/plain"
OUT=$(probe guard_install "$R" "expected@corp.test")
ok "installs the pre-commit guard" "$(printf '%s' "$OUT" | jqf message)" "Guard installed"
cd "$R"; echo x >> src/a.txt; git add -A >/dev/null
BLOCKED=$(git commit -m "wrong identity" 2>&1); RC=$?
ok "  a mismatched commit is blocked" "$BLOCKED" "GitSwitch: commit blocked"
ok "  with a non-zero exit" "rc=$RC" "rc=1"
OUT=$(probe guard_uninstall "$R")
ok "uninstalls cleanly" "$(printf '%s' "$OUT" | jqf message)" "removed"
git commit -qm "now allowed" 2>/dev/null
ok "  and commits work again" "$(git -C "$R" log -1 --format=%s)" "now allowed"

section "Guard chains a foreign hook instead of replacing it"
cat > "$R/.git/hooks/pre-commit" <<'HOOK'
#!/bin/sh
echo "FOREIGN HOOK RAN" >&2
exit 0
HOOK
chmod +x "$R/.git/hooks/pre-commit"
OUT=$(probe guard_install "$R" "regress@example.test")
ok "the existing hook is preserved" "$(printf '%s' "$OUT" | jqf message)" "existing hook preserved"
ok "  saved alongside" "$([ -f "$R/.git/hooks/pre-commit.gitswitch-saved" ] && echo yes || echo no)" "yes"
echo y >> "$R/src/a.txt"; git -C "$R" add -A >/dev/null
CHAINED=$(git -C "$R" commit -m "chained" 2>&1)
ok "  and it still runs on commit" "$CHAINED" "FOREIGN HOOK RAN"
probe guard_uninstall "$R" >/dev/null
ok "uninstall restores the foreign hook" "$(cat "$R/.git/hooks/pre-commit")" "FOREIGN HOOK RAN"

section "History: branches, graph page, sync"
OUT=$(probe branches "$R")
ok "lists local and remote branches" "$OUT" "feature"
ok "  marks the current one" "$(printf '%s' "$OUT" | python3 -c "import sys,json;print([b['name'] for b in json.load(sys.stdin) if b['is_current']])")" "main"
OUT=$(probe hpage "$R")
ok "returns a page of commits" "$(printf '%s' "$OUT" | jqf total)" "5"
ok "  with graph lanes assigned" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print('lane' if 'lane' in d['commits'][0] else 'missing')")" "lane"
OUT=$(probe sync "$R" "main")
ok "sync reports ahead/behind against the upstream" "$(printf '%s' "$OUT" | jqf upstream)" "origin/main"

section "Check GitHub (peek): what is new on the remote, without downloading"
W="$SB/clones/plain"
git -C "$W" fetch -q origin 2>/dev/null
OBJ_BEFORE="$(git -C "$W" count-objects -v | tr '\n' ' ')"; REF_BEFORE="$(git -C "$W" rev-parse refs/remotes/origin/main)"
mtime() { "$PY" -c 'import os,sys; print(os.path.getmtime(sys.argv[1]) if os.path.exists(sys.argv[1]) else "none")' "$1"; }
FH_BEFORE="$(mtime "$W/.git/FETCH_HEAD")"
OUT=$(probe peek "$W")
ok "nothing new is reported as such" "$(printf '%s' "$OUT" | jqf changed)" "False"
ok "  in words" "$(printf '%s' "$OUT" | jqf message)" "Nothing new on origin/main"
cd "$SB/seed"; git pull -q --rebase origin main 2>/dev/null; echo more >> src/a.txt; git add -A >/dev/null; git commit -qm "seed: more"; git push -q origin main 2>/dev/null
OUT=$(probe peek "$W")
ok "a commit pushed elsewhere shows the branch moved" "$(printf '%s' "$OUT" | jqf changed)" "True"
ok "  naming the new tip" "$(printf '%s' "$OUT" | jqf remote_tip)" "$(git -C "$SB/seed" rev-parse HEAD)"
ok "  the objects are not here, so no count is invented" "$(printf '%s' "$OUT" | jqf new_commits)" "None"
ok "  and it says nothing was downloaded" "$(printf '%s' "$OUT" | jqf message)" "Nothing was downloaded"
ok "  origin/main did not move" "$(git -C "$W" rev-parse refs/remotes/origin/main)" "$REF_BEFORE"
ok "  no object arrived" "$(git -C "$W" count-objects -v | tr '\n' ' ')" "$OBJ_BEFORE"
ok "  FETCH_HEAD was not written" "$(mtime "$W/.git/FETCH_HEAD")" "$FH_BEFORE"
git -C "$W" fetch -q origin 2>/dev/null
OUT=$(probe peek "$W")
ok "after a fetch the check is quiet again" "$(printf '%s' "$OUT" | jqf changed)" "False"
git -C "$W" reset -q --hard origin/main~1
git -C "$W" branch -q --set-upstream-to=origin/main
git -C "$W" update-ref refs/remotes/origin/main origin/main~1
OUT=$(probe peek "$W")
ok "when the objects are already here the count is exact" "$(printf '%s' "$OUT" | jqf new_commits):$(printf '%s' "$OUT" | jqf counted_by)" "1:local"
git -C "$W" fetch -q origin 2>/dev/null; git -C "$W" reset -q --hard origin/main
git -C "$SB/seed" push -q origin :refs/heads/gone 2>/dev/null; git -C "$SB/seed" push -q origin main:gone 2>/dev/null; git -C "$W" fetch -q origin 2>/dev/null; git -C "$W" switch -q -c gone --track origin/gone; git -C "$SB/seed" push -q origin :refs/heads/gone 2>/dev/null
OUT=$(probe peek "$W")
ok "a branch deleted on the remote is reported, not guessed at" "$(printf '%s' "$OUT" | jqf branch_gone)" "True"
git -C "$W" switch -q main

section "The Changes page still reads these repos correctly"
OUT=$(probe status "$SB/clones/withsub")
ok "status sees the submodule repo" "$(printf '%s' "$OUT" | jqf has_submodules)" "True"
ok "  on the right branch" "$(printf '%s' "$OUT" | jqf branch)" "main"
OUT=$(probe status "$SB/clones/sparse")
ok "status works on a sparse checkout too" "$(printf '%s' "$OUT" | jqf name)" "sparse"

verify_result
