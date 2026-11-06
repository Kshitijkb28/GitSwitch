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

# What the Clone page was shown meanwhile: the submodule named while it was
# being fetched, then the finished count — and no large-files count, since
# nothing was asked for.
TRACE=$(probe_trace)
ok "  the progress line named the submodule while it was fetched" "$(printf '%s' "$TRACE" | "$PY" -c 'import sys,json;p=[p for p in json.load(sys.stdin) if p["phase"]=="Submodules" and p.get("done") is not None][0];print(p["done"],"of",p["total"],p["detail"])')" "0 of 1 vendor/sub"
ok "  and the last word on the submodules was all of them done, nothing named" "$(printf '%s' "$TRACE" | "$PY" -c 'import sys,json;p=[p for p in json.load(sys.stdin) if p["phase"]=="Submodules"][-1];print(p["phase"],p["done"],"of",p["total"],p["percent"],p.get("detail"))')" "Submodules 1 of 1 100 None"
ok "  no large-files count was claimed" "$(printf '%s' "$TRACE" | "$PY" -c 'import sys,json;print([p for p in json.load(sys.stdin) if p["phase"]=="Large files" and p.get("done") is not None])')" "[]"

section "Sparse clone"
OUT=$(probe sparse_clone "" "$SB/origin.git,$SB/clones,sparse")
ok "sparse clone succeeds" "$(printf '%s' "$OUT" | jqf path)" "$SB/clones/sparse"
OUT=$(probe sparse_info "$SB/clones/sparse")
ok "  lists the repo's top-level folders" "$(printf '%s' "$OUT" | jqf available_dirs)" "docs"
ok "  and reports the branch" "$(printf '%s' "$OUT" | jqf branch)" "main"

section "Sparse clone: folder names git would quote"
git init -q --bare "$SB/names.git"; git clone -q "$SB/names.git" "$SB/names-author" 2>/dev/null
mkdir -p "$SB/names-author/café" "$SB/names-author/docs"; printf 'x\n' > "$SB/names-author/café/x.txt"; printf 'r\n' > "$SB/names-author/docs/readme.md"
IS_WIN=0; case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) IS_WIN=1 ;; esac
if [ "$IS_WIN" = 0 ]; then mkdir -p "$SB/names-author/say\"hi"; printf 'q\n' > "$SB/names-author/say\"hi/q.txt"; fi
git -C "$SB/names-author" add -A >/dev/null; git -C "$SB/names-author" commit -qm names; git -C "$SB/names-author" push -q origin main 2>/dev/null
OUT=$(probe sparse_clone "" "$SB/names.git,$SB/clones,names")
INFO=$(probe sparse_info "$SB/clones/names")
ok "a folder with an accented name is listed as it is on disk" "$(printf '%s' "$INFO" | "$PY" -c 'import sys,json;sys.stdout.reconfigure(encoding="utf-8");print("café" in json.load(sys.stdin)["available_dirs"])')" "True"
OUT=$(probe sparse_set "$SB/clones/names" "café")
ok "  and ticking it checks it out" "$(cat "$SB/clones/names/café/x.txt" 2>/dev/null || echo MISSING)" "x"
ok "  and the selection reads back in the same spelling" "$(probe sparse_info "$SB/clones/names" | "$PY" -c 'import sys,json;sys.stdout.reconfigure(encoding="utf-8");print(json.load(sys.stdin)["sparse_dirs"])')" "['café']"
if [ "$IS_WIN" = 0 ]; then
  OUT=$(probe sparse_set "$SB/clones/names" 'café:say"hi')
  ok "  a folder with a quote in its name too" "$(cat "$SB/clones/names/say\"hi/q.txt" 2>/dev/null || echo MISSING):$(probe sparse_info "$SB/clones/names" | "$PY" -c 'import sys,json;sys.stdout.reconfigure(encoding="utf-8");print(sorted(json.load(sys.stdin)["sparse_dirs"]))')" "q:['café', 'say\"hi']"
fi

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
OUT=$(probe guard_install "$R" "regress@example.test")
echo z >> "$R/src/a.txt"; git -C "$R" add -A >/dev/null
CHAINED=$(git -C "$R" commit -m "chained again" 2>&1)
ok "  installing over its own guard keeps the chained hook" "$CHAINED" "FOREIGN HOOK RAN"
ok "  the guard names the email it expects, where the app reads it" "$(probe guard_email "$R")" "regress@example.test"
# A profile's email may hold a quote, a dollar sign or a backtick: the shell must take it literally.
OUT=$(probe guard_install "$R" 'q"$HOME`x@example.test')
echo w >> "$R/src/a.txt"; git -C "$R" add -A >/dev/null
ok "an email the shell would expand is held exactly" "$(git -C "$R" commit -m "odd email" 2>&1)" 'should commit as: q"$HOME`x@example.test'
ok "  and read back exactly" "$(probe guard_email "$R")" 'q\"$HOME`x@example.test'
# Back to the history the sections below count.
git -C "$R" reset -q --hard HEAD~1
probe guard_uninstall "$R" >/dev/null
ok "uninstall restores the foreign hook" "$(cat "$R/.git/hooks/pre-commit")" "FOREIGN HOOK RAN"

section "History: branches, graph page, sync"
OUT=$(probe branches "$R")
ok "lists local and remote branches" "$OUT" "feature"
ok "  marks the current one" "$(printf '%s' "$OUT" | python3 -c "import sys,json;print([b['name'] for b in json.load(sys.stdin) if b['is_current']])")" "main"
OUT=$(probe hpage "$R")
ok "returns a page of commits" "$(printf '%s' "$OUT" | jqf total)" "5"
ok "  with graph lanes assigned" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print('lane' if 'lane' in d['commits'][0] else 'missing')")" "lane"
ok "  and the author date beside the committer date" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);c=d['commits'][0];print(c['author_date'][:4].isdigit() and c['date'][:4].isdigit())")" "True"
OUT=$(probe hpage "$SB/clones/withsub")
ok "a commit that adds a submodule says which pointer it touches" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([c['touches_submodules'] for c in d['commits'] if 'vendor/sub' in c['touches_submodules']][:1])")" "[['vendor/sub']]"
ok "  but is not a pointer-only bump: it adds .gitmodules too" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([c['only_submodules'] for c in d['commits'] if 'vendor/sub' in c['touches_submodules']][:1])")" "[False]"
git -C "$SB/clones/withsub/vendor/sub" commit -q --allow-empty -m "sub: bump"; git -C "$SB/clones/withsub" add vendor/sub; git -C "$SB/clones/withsub" commit -qm "ci: bump vendor/sub"
OUT=$(probe hpage "$SB/clones/withsub")
ok "a commit that only moves the pointer is a bump" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);c=d['commits'][0];print(c['subject'],c['only_submodules'],c['touches_submodules'])")" "ci: bump vendor/sub True ['vendor/sub']"
OUT=$(probe hpage "$SB/clones/withsub" "hide-pointer-only")
ok "  and hiding pointer-only commits leaves it out of the list and the count" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print(any(c['subject']=='ci: bump vendor/sub' for c in d['commits']), d['total'])")" "False $(( $(git -C "$SB/clones/withsub" rev-list --count HEAD) - 1 ))"
# The author's date and the committer's are two facts, each read from its own field.
AD="$SB/dates"; git init -q -b main "$AD"
GIT_AUTHOR_DATE="2020-01-02T03:04:05+00:00" GIT_COMMITTER_DATE="2021-06-07T08:09:10+00:00" git -C "$AD" commit -q --allow-empty -m "two dates"
OUT=$(probe hpage "$AD")
ok "the author date and the committer date are told apart" "$(printf '%s' "$OUT" | python3 -c "import sys,json;c=json.load(sys.stdin)['commits'][0];print(c['author_date'][:10],c['date'][:10])")" "2020-01-02 2021-06-07"
# People: an address matches whole and as written; several are any of them.
for who in "me@x.test" "xme@x.test" "someone@x.testing" "1+me@x.test" "ci[bot]@x.test"; do git -C "$AD" -c user.name="N" -c user.email="$who" commit -q --allow-empty -m "by $who"; done
by() { local ARGS="{\"repoPath\":\"$AD\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":50,\"search\":null,\"authors\":$1,\"hidePointerOnly\":false}"; pinvoke history_page "$ARGS" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['total'], sorted(c['subject'] for c in d['commits']))"; }
ONE=$(by '["me@x.test"]'); PLUS=$(by '["1+me@x.test"]'); BOT=$(by '["ci[bot]@x.test"]'); TWO=$(by '["me@x.test","someone@x.testing"]'); NONE=$(by '[]')
ok "one address matches its own commits, not a longer one ending the same" "$ONE" "1 ['by me@x.test']"
ok "  a plus in an address is literal" "$PLUS" "1 ['by 1+me@x.test']"
ok "  and so are brackets" "$BOT" "1 ['by ci[bot]@x.test']"
ok "  several addresses are any of them" "$TWO" "2 ['by me@x.test', 'by someone@x.testing']"
ok "  and an empty list is everyone" "$NONE" "6 ["
# A path that was an ordinary folder before it became a submodule: hiding the
# pointer must not hide the commits that worked on the files it used to hold.
H="$SB/hist"; git init -q -b main "$H"; git init -q -b main "$SB/hsub"; git -C "$SB/hsub" commit -q --allow-empty -m s1
mkdir -p "$H/vendor/lib"; printf 'c\n' > "$H/vendor/lib/code.c"; git -C "$H" add -A >/dev/null; git -C "$H" commit -qm "add vendored lib"
printf 'c2\n' >> "$H/vendor/lib/code.c"; git -C "$H" commit -qam "work inside the vendored folder"
git -C "$H" rm -q -r vendor/lib; git -C "$H" commit -qm "remove the vendored folder"
( cd "$H" && git submodule add -q "$SB/hsub" vendor/lib 2>/dev/null ); git -C "$H" commit -qm "vendor/lib becomes a submodule"
git -C "$H/vendor/lib" commit -q --allow-empty -m s2; git -C "$H" add vendor/lib; git -C "$H" commit -qm "bump vendor/lib"
OUT=$(probe hpage "$H" "hide-pointer-only")
ok "hiding pointer-only commits hides the bump" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print(any(c['subject']=='bump vendor/lib' for c in d['commits']))")" "False"
ok "  and nothing that touched files there while it was a folder" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print(sorted(c['subject'] for c in d['commits']))")" "['add vendored lib', 'remove the vendored folder', 'vendor/lib becomes a submodule', 'work inside the vendored folder']"
ok "  the count is of those rows" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['total'],len(d['commits']))")" "4 4"
# A pointer change is a bump whatever the ignore setting says; a merge never is; a name is read as it is.
git -C "$H" config -f .gitmodules submodule.vendor/lib.ignore all; git -C "$H" commit -qam "ignore = all"
git -C "$H/vendor/lib" commit -q --allow-empty -m s3
git -C "$H" update-index --cacheinfo "160000,$(git -C "$H/vendor/lib" rev-parse HEAD),vendor/lib"; git -C "$H" commit -qm "bump again"
( cd "$H" && git submodule add -q "$SB/hsub" "módulo" 2>/dev/null ); git -C "$H" commit -qm "add módulo"
git -C "$H/módulo" commit -q --allow-empty -m s4; git -C "$H" add "módulo"; git -C "$H" commit -qm "bump módulo"
git -C "$H" switch -q -c side; printf 's\n' > "$H/side.txt"; git -C "$H" add side.txt; git -C "$H" commit -qm "side work"
git -C "$H" switch -q main; printf 'm\n' > "$H/main.txt"; git -C "$H" add main.txt; git -C "$H" commit -qm "main work"; git -C "$H" merge -q --no-ff -m "merge side" side
OUT=$(probe hpage "$H")
ok "a bump of a submodule git is told to ignore is still a bump" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([(c['only_submodules'],c['touches_submodules']) for c in d['commits'] if c['subject']=='bump again'])")" "[(True, ['vendor/lib'])]"
ok "  a path with a non-ASCII name is recognised" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([c['only_submodules'] for c in d['commits'] if c['subject'].startswith('bump m')], [len(c['touches_submodules']) for c in d['commits'] if c['subject'].startswith('bump m')])")" "[True] [1]"
ok "  a merge is never one" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([(c['only_submodules'],c['touches_submodules']) for c in d['commits'] if c['is_merge']])")" "[(False, [])]"
# One person under two addresses: the total, the rows and the people list agree.
MM="$SB/mailmap"; git init -q -b main "$MM"
for i in 1 2 3; do git -C "$MM" -c user.name=Old -c user.email=old@corp.test commit -q --allow-empty -m "old $i"; done
for i in 1 2; do git -C "$MM" -c user.name=Me -c user.email=me@new.test commit -q --allow-empty -m "new $i"; done
printf 'Me <me@new.test> <old@corp.test>\n' > "$MM/.mailmap"; git -C "$MM" add .mailmap; git -C "$MM" -c user.name=Me -c user.email=me@new.test commit -qm "mailmap"
OUT=$(pinvoke history_page "{\"repoPath\":\"$MM\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":4,\"search\":null,\"authors\":[\"me@new.test\"],\"hidePointerOnly\":false}")
ok "with a .mailmap, one person's total counts every address of theirs" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['total'],len(d['commits']),d['has_more'])")" "6 4 True"
ok "  the rows carry the mapped name and address" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(sorted(set((c['author_name'],c['author_email']) for c in d['commits'])))")" "[('Me', 'me@new.test')]"
PEOPLE=$(pinvoke history_authors "{\"repoPath\":\"$MM\",\"rev\":\"HEAD\"}")
ok "  and the people list says the same number" "$(printf '%s' "$PEOPLE" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print([(a['email'],a['commits']) for a in d])")" "[('me@new.test', 6)]"
OUT=$(pinvoke history_page "{\"repoPath\":\"$MM\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":4,\"search\":null,\"authors\":[\"old@corp.test\"],\"hidePointerOnly\":false}")
ok "  the old address alone matches nothing, and says so" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['total'],len(d['commits']),d['has_more'])")" "0 0 False"
# The same with log.mailmap switched off here: the total and the rows are still the mapped person's.
git -C "$MM" config log.mailmap false
OUT=$(pinvoke history_page "{\"repoPath\":\"$MM\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":4,\"search\":null,\"authors\":[\"me@new.test\"],\"hidePointerOnly\":false}")
ok "  with log.mailmap=false in the repository, the total and the rows do not change" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['total'],len(d['commits']),d['has_more'],sorted(set(c['author_email'] for c in d['commits'])))")" "6 4 True ['me@new.test']"
git -C "$MM" config --unset log.mailmap
# People whose commits carry no address, or one that is not an address: picked by their whole identity.
NA="$SB/noaddr"; git init -q -b main "$NA"
who() { GIT_AUTHOR_NAME="$1" GIT_AUTHOR_EMAIL="$2" git -C "$NA" commit -q --allow-empty -m "$3"; }
who "Sam" "" "sam 1"; who "Sam" "" "sam 2"; who "Samantha Lee" "samantha@x.test" "samantha"; who "Other" "osama@x.test" "other"; who "jdoe" "jdoe" "imported"
who "Uncle Sam" "" "uncle sam"; who "bjdoe" "jdoe" "another import"; who "A.B [bot]" "" "a bot without an address"
PEOPLE=$(pinvoke history_authors "{\"repoPath\":\"$NA\",\"rev\":\"HEAD\"}")
ok "a person without an address is listed with none" "$(printf '%s' "$PEOPLE" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(sorted((a['name'],a['email'],a['commits']) for a in d))")" "('Sam', '', 2)"
whole() { local ARGS="{\"repoPath\":\"$NA\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":50,\"search\":null,\"authors\":$1,\"hidePointerOnly\":false}"; pinvoke history_page "$ARGS" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['total'], sorted(c['subject'] for c in d['commits']))"; }
SAM=$(whole '["Sam <>"]'); JD=$(whole '["jdoe <jdoe>"]')
ok "  picked by the whole identity, it is only theirs — not everyone whose name or address contains or ends with it" "$SAM" "2 ['sam 1', 'sam 2']"
ok "  and so is an address that is not an address" "$JD" "1 ['imported']"
ok "  dots and brackets in such an identity are read as themselves" "$(whole '["A.B [bot] <>"]')" "1 ['a bot without an address']"
ok "  an address in its brackets alone is that address, under any name" "$(whole '["<jdoe>"]')" "2 ['another import', 'imported']"
search() { local ARGS="{\"repoPath\":\"$NA\",\"rev\":\"HEAD\",\"offset\":0,\"limit\":50,\"search\":\"$1\",\"authors\":[],\"hidePointerOnly\":false}"; pinvoke history_page "$ARGS" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['total'], sorted(c['subject'] for c in d['commits']))"; }
ok "  a search is still the text itself: a dot is a dot" "$(search 'sam .')" "0 []"
ok "    and the text is found whatever its case" "$(search 'SAM 1')" "1 ['sam 1']"
# Signed commits, with git told to show signatures in every log: the names read are still only names.
if command -v ssh-keygen >/dev/null 2>&1; then
  SG="$SB/signed"; mkdir -p "$SG/keys"; ssh-keygen -q -t ed25519 -N "" -C sig@x.test -f "$SG/keys/id" >/dev/null 2>&1
  printf 'sig@x.test %s\n' "$(cat "$SG/keys/id.pub")" > "$SG/keys/allowed"
  git init -q -b main "$SG/sub"; git -C "$SG/sub" commit -q --allow-empty -m s1
  git init -q -b main "$SG/repo"
  for k in gpg.format=ssh "user.signingkey=$SG/keys/id.pub" commit.gpgsign=true "gpg.ssh.allowedSignersFile=$SG/keys/allowed" user.email=sig@x.test; do git -C "$SG/repo" config "${k%%=*}" "${k#*=}"; done
  printf 'r\n' > "$SG/repo/r.txt"; git -C "$SG/repo" add r.txt
  if git -C "$SG/repo" commit -qm "base" 2>/dev/null; then
    ( cd "$SG/repo" && git submodule add -q "$SG/sub" libs/sub 2>/dev/null ); git -C "$SG/repo" commit -qm "add sub"
    for n in 1 2; do git -C "$SG/repo/libs/sub" commit -q --allow-empty -m "s$((n+1))"; git -C "$SG/repo" add libs/sub; git -C "$SG/repo" commit -qm "BUMP $n"; done
    printf 'r2\n' >> "$SG/repo/r.txt"; git -C "$SG/repo" commit -qam "file change"
    ok "the commits are signed, and git verifies them" "$(git -C "$SG/repo" log --format=%G? | tr -d '\n')" "GGGGG"
    git -C "$SG/repo" config log.showSignature true
    ok "  a plain log now carries the signature check" "$(git -C "$SG/repo" log -1 --format=%s | grep -c 'signature' || true)" "1"
    OUT=$(probe hpage "$SG/repo")
    ok "with log.showSignature on, a signed pointer bump is still read as one" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([(c['subject'],c['only_submodules'],c['touches_submodules']) for c in d['commits'] if c['subject'].startswith('BUMP')])")" "[('BUMP 2', True, ['libs/sub']), ('BUMP 1', True, ['libs/sub'])]"
    ok "  and the rows are the commits, nothing of the check among them" "$(printf '%s' "$OUT" | python3 -c "import sys,json;d=json.load(sys.stdin);print([c['subject'] for c in d['commits']])")" "['file change', 'BUMP 2', 'BUMP 1', 'add sub', 'base']"
    SGH="$(git -C "$SG/repo" rev-parse HEAD)"
    DET=$(pinvoke history_commit_detail "{\"repoPath\":\"$SG/repo\",\"hash\":\"$SGH\"}")
    ok "  a commit's own page reads its hash and files, not the check" "$(printf '%s' "$DET" | python3 -c "import sys,json;d=json.load(sys.stdin)['value'];print(d['hash']=='$SGH',d['subject'],[f['path'] for f in d['files']])")" "True file change ['r.txt']"
  else
    printf '  SKIPPED: this git cannot sign with an ssh key, so the signature section has nothing to read\n'
  fi
else
  printf '  SKIPPED: no ssh-keygen here, so the signature section has nothing to sign with\n'
fi
OUT=$(probe sync "$R" "main")
ok "sync reports ahead/behind against the upstream" "$(printf '%s' "$OUT" | jqf upstream)" "origin/main"
git -C "$SB/clones/withsub/vendor/sub" fetch -q origin 2>/dev/null
OUT=$(probe sync "$SB/clones/withsub/vendor/sub" "")
ok "  a submodule's fetch age is read through its gitfile, not a missing .git folder" "$(printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin);v=d.get('last_fetch_secs');print('known' if isinstance(v,int) else v)")" "known"

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

section "History marks what no remote-tracking branch has, and can list only that"
UP="$SB/unpushed"; git init -q --bare "$UP.git"; git clone -q "$UP.git" "$UP" 2>/dev/null
for n in 1 2; do printf '%s\n' "$n" > "$UP/f$n.txt"; git -C "$UP" add -A >/dev/null; git -C "$UP" commit -qm "pushed $n"; done
git -C "$UP" push -q -u origin main 2>/dev/null
for n in 3 4; do printf '%s\n' "$n" > "$UP/f$n.txt"; git -C "$UP" add -A >/dev/null; git -C "$UP" commit -qm "local $n"; done
hp() { pinvoke history_page "{\"repoPath\":\"$1\",\"rev\":\"$2\",\"offset\":0,\"limit\":50,\"search\":null,\"authors\":null,\"hidePointerOnly\":false,\"unpushedOnly\":$3}"; }
ok "each row says whether a remote-tracking branch has it" "$(hp "$UP" HEAD false | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];print(d["remote_refs"], [(c["subject"],c["unpushed"]) for c in d["commits"]])')" "True [('local 4', True), ('local 3', True), ('pushed 2', False), ('pushed 1', False)]"
ok "  the filter lists only those, with their count" "$(hp "$UP" HEAD true | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];print(d["total"], [c["subject"] for c in d["commits"]], all(c["unpushed"] for c in d["commits"]))')" "2 ['local 4', 'local 3'] True"
# Pushed to any remote counts: the commit exists somewhere else than here.
git init -q --bare "$UP-other.git"; git -C "$UP" remote add other "$UP-other.git"; git -C "$UP" push -q other main 2>/dev/null
ok "  a push to any remote counts" "$(hp "$UP" HEAD true | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];print(d["total"])')" "0"
git -C "$UP" switch -q -c topic 2>/dev/null; printf 't\n' > "$UP/t.txt"; git -C "$UP" add -A >/dev/null; git -C "$UP" commit -qm "topic only"
ok "  across all branches too" "$(hp "$UP" --all true | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];print(d["total"], [c["subject"] for c in d["commits"]])')" "1 ['topic only']"
# All branches are branches, remote-tracking branches, tags and HEAD: not the
# stash, notes, a preview's parked refs, git's prefetch refs, or GitSwitch's
# own backup branches (an Update's, under today's name and the one an earlier
# build gave it, and a reset's) — none is work to push, and each was marked
# not pushed.
printf 's\n' > "$UP/s.txt"; git -C "$UP" add s.txt; git -C "$UP" commit -qm "stale copy"
git -C "$UP" branch gitswitch-before-update-1; git -C "$UP" branch gitswitch-before-sync-1; git -C "$UP" branch gitswitch-before-reset-20261008000000; git -C "$UP" reset -q --hard HEAD~1
printf 'w\n' >> "$UP/t.txt"; git -C "$UP" stash -q
git -C "$UP" update-ref refs/prefetch/remotes/origin/main "$(git -C "$UP" commit-tree -p HEAD -m 'fetched in the background' 'HEAD^{tree}')"
git -C "$UP" update-ref refs/gitswitch-moved/origin/main "$(git -C "$UP" commit-tree -p HEAD -m 'parked by a preview' 'HEAD^{tree}')"
git -C "$UP" notes add -m 'a note' HEAD
WANT_ALL="$(git -C "$UP" rev-list --count --exclude='gitswitch-before-*' --branches --remotes --tags HEAD)"
ok "  all branches leave out GitSwitch's backups, the stash, parked, prefetch and notes refs: only the branch's own commit is unpushed" "$(hp "$UP" --all true | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];print(d["total"], [c["subject"] for c in d["commits"]])')" "1 ['topic only']"
ok "    and none of them is in the full list either" "$(hp "$UP" --all false | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];bad=[c["subject"] for c in d["commits"] if c["subject"] in ("stale copy","fetched in the background","parked by a preview") or c["subject"].startswith(("WIP on","index on","Notes added"))];print(d["total"]==int(sys.argv[1]), bad)' "$WANT_ALL")" "True []"
AUTH=$(pinvoke history_authors "{\"repoPath\":\"$UP\",\"rev\":\"--all\"}")
ok "    and the People list counts the same commits" "$(printf '%s' "$AUTH" | "$PY" -c 'import sys,json;print(sum(a["commits"] for a in json.load(sys.stdin)["value"])==int(sys.argv[1]))' "$WANT_ALL")" "True"
ok "  a backup still opens when it is chosen by name" "$(hp "$UP" gitswitch-before-update-1 false | "$PY" -c 'import sys,json;print("stale copy" in [c["subject"] for c in json.load(sys.stdin)["value"]["commits"]])')" "True"
# Paging: the second page is the next fifty, in the same order as the first.
PG="$SB/paging"; git init -q "$PG"; for i in $(seq 1 60); do git -C "$PG" commit -q --allow-empty -m "c$i"; done
P2=$(pinvoke history_page "{\"repoPath\":\"$PG\",\"rev\":\"HEAD\",\"offset\":50,\"limit\":50,\"search\":null,\"authors\":null,\"hidePointerOnly\":false,\"unpushedOnly\":false}")
ok "the second page is the next fifty in order" "$(printf '%s' "$P2" | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];print(len(d["commits"]), d["has_more"], d["commits"][0]["hash"]==sys.argv[1])' "$(git -C "$PG" log --date-order --skip=50 -1 --format=%H HEAD)")" "10 False True"
NR="$SB/noremote"; git init -q "$NR"; printf 'x\n' > "$NR/x.txt"; git -C "$NR" add -A >/dev/null; git -C "$NR" commit -qm "alone"
ok "without a remote-tracking branch nothing is marked, and the page says there is none to compare with" "$(hp "$NR" HEAD false | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["value"];print(d["remote_refs"], [c["unpushed"] for c in d["commits"]])')" "False [False]"

section "A folder a clone is still writing is listed as cloning, not as on this Mac"
# Last: the profile made here becomes the default, which every later commit would carry.
mkdir -p "$SB/profiled"
"$PY" - "$SB/pprof.json" "$SB/profiled" <<'JSONPY'
import json, sys
json.dump({"name": "Profiled", "git_name": "P", "git_email": "p@example.test", "directories": [sys.argv[2]]}, open(sys.argv[1], "w"))
JSONPY
probe profile_apply "$SB" "$SB/pprof.json" >/dev/null
git init -q "$SB/profiled/wip"; git -C "$SB/profiled/wip" remote add origin git@github.com:someone/wip.git
git init -q "$SB/profiled/existing"; git -C "$SB/profiled/existing" remote add origin git@github.com:someone/existing.git
OUT=$(probe local_clones_while_cloning "" "$SB/profiled/wip,git@github.com:someone/wip.git")
ok "a folder a clone is still writing is listed as cloning, not as on this Mac" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["during"];print("someone/wip" in d["cloning"], "someone/wip" in d["clones"])')" "True False"
ok "  and the Repository page's picker does not offer it — though it offers the one beside it" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;ps=[p.replace("\\","/") for p in json.load(sys.stdin)["picker"]];print(any(p.endswith("/wip") for p in ps), any(p.endswith("/existing") for p in ps))')" "False True"
ok "  the repository beside it is listed as usual" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print("someone/existing" in json.load(sys.stdin)["during"]["clones"])')" "True"
ok "once the clone is over it is on this Mac" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;d=json.load(sys.stdin)["after"];print("someone/wip" in d["clones"], len(d["cloning"]))')" "True 0"
# A profile folder stored in another letter case than the disk spells it
# (where the file system ignores case): the clone running into it is still
# listed as cloning.
mkdir -p "$SB/Case-Probe"
if [ -d "$SB/case-probe" ]; then
  mkdir -p "$SB/CasedClones"
  "$PY" - "$SB/pcasedclones.json" "$SB/casedclones" <<'JSONPY'
import json, sys
json.dump({"name": "CasedClones", "git_name": "C", "git_email": "c@example.test", "directories": [sys.argv[2]]}, open(sys.argv[1], "w"))
JSONPY
  probe profile_apply "$SB" "$SB/pcasedclones.json" >/dev/null
  # Stored exactly as typed (an older build, or a hand edit): the app itself
  # now saves the disk's spelling.
  STORE_R="$("$PY" -c 'import os;h=os.environ["HOME"];c=[os.path.join(h,"Library/Application Support/com.gitswitch.app/profiles.json"),os.path.join(h,".local/share/com.gitswitch.app/profiles.json"),os.path.join(h,"AppData/Roaming/com.gitswitch.app/profiles.json")];print(next((p for p in c if os.path.exists(p)),"MISSING"))')"
  "$PY" - "$STORE_R" "$SB/casedclones" <<'STOREPY'
import json, sys
f, folder = sys.argv[1:]
d = json.load(open(f))
for p in d["profiles"]:
    if p["name"] == "CasedClones":
        p["directories"] = [folder]
json.dump(d, open(f, "w"))
STOREPY
  OUT=$(probe local_clones_while_cloning "" "$SB/CasedClones/wip2,git@github.com:someone/wip2.git")
  ok "a clone into a folder stored in another letter case is still listed as cloning" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print("someone/wip2" in json.load(sys.stdin)["during"]["cloning"])')" "True"
fi

verify_result
