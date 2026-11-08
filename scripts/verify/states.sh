#!/bin/bash
# States suite, against real git: save where a repository is (commit, branch,
# staged, every changed and new file, the repositories inside it) and go back
# to it byte for byte. The user's example first (commit 1 + 3 changed files,
# two more commits, back), then what must never be lost: commits that leave a
# branch, files made since, ignored files, files ignored only now; pushed
# commits (refused, a new branch, or anyway when asked); a detached HEAD; a
# deleted branch; another worktree; a merge in progress; the four slots;
# CRLF and binary files; submodules and nested repositories at any depth.
# Throwaway repos, an isolated HEAD, local bare repos as the "remotes".
set -u
source "$(dirname "$0")/lib.sh"
verify_init states "States Tester" "states@example.test"
SB="$(realdir "$SB")"

hash() { if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d' ' -f1; else shasum -a 256 | cut -d' ' -f1; fi; }
# Everything a state must bring back, as one hash: the status, both diffs
# (staged and not), the index, and the bytes of every file that isn't ignored.
fingerprint() {
  (
    cd "$1" || exit 1
    git status --porcelain=v1 --untracked-files=all
    git diff --cached --binary
    git diff --binary
    git ls-files -s
    git ls-files -z -co --exclude-standard | while IFS= read -r -d '' f; do
      if [ -f "$f" ]; then printf '%s %s\n' "$f" "$(sha256file "$f")"; else printf '%s missing\n' "$f"; fi
    done
  ) | hash
}
list() { pinvoke states_list "{\"repoPath\":\"$1\"}"; }
save() { pinvoke states_save "{\"repoPath\":\"$1\",\"slot\":\"$2\",\"name\":\"$3\",\"replace\":${4:-null}}"; }
back() { pinvoke states_restore "{\"repoPath\":\"$1\",\"slot\":\"$2\",\"oid\":\"$3\",\"newBranch\":${4:-null},\"allowPushed\":${5:-false}}"; }
del() { pinvoke states_delete "{\"repoPath\":\"$1\",\"slot\":\"$2\",\"oid\":\"$3\"}"; }
oid_of() {
  list "$1" | "$PY" -c 'import sys,json
v=json.load(sys.stdin)["value"]; s=sys.argv[1]
st=v["before"] if s=="before" else next((x for x in v["states"] if x["slot"]==s), None)
print(st["oid"] if st else "")' "$2"
}
head_of() { git -C "$1" rev-parse HEAD; }
on_branch() { git -C "$1" symbolic-ref -q --short HEAD || echo detached; }
there() { [ -e "$1" ] && echo there || echo gone; }
# A repository with "commit 1" (a.txt b.txt c.txt, .env and build/ ignored), pushed.
fresh() {
  git init -q --bare "$SB/$1.git"
  git clone -q "$SB/$1.git" "$SB/$1" 2>/dev/null
  printf 'a1\n' > "$SB/$1/a.txt"; printf 'b1\n' > "$SB/$1/b.txt"; printf 'c1\n' > "$SB/$1/c.txt"
  printf '.env\nbuild/\n' > "$SB/$1/.gitignore"
  git -C "$SB/$1" add -A >/dev/null; git -C "$SB/$1" commit -qm "commit 1"
  git -C "$SB/$1" push -q -u origin main 2>/dev/null
}

section "Your example: commit 1 with 3 changed files, two more commits, then back"
fresh ex; W="$SB/ex"; C1=$(head_of "$W")
printf 'SECRET=1\n' > "$W/.env"; mkdir -p "$W/build"; printf 'x' > "$W/build/x"
# a.txt staged and changed again, b.txt changed, c.txt deleted, d.txt new.
printf 'a2\n' > "$W/a.txt"; git -C "$W" add a.txt; printf 'a3\n' >> "$W/a.txt"
printf 'b2\n' > "$W/b.txt"; rm "$W/c.txt"; printf 'd new\n' > "$W/d.txt"
FP1=$(fingerprint "$W")
OUT=$(save "$W" 1 "my first stage")
ok "it is saved into slot 1" "$(printf '%s' "$OUT" | jqf value.ok)" "True"
ok "  in words: the branch, the commit, the changes" "$(printf '%s' "$OUT" | jqf value.detail)" "main at"
ok "  and saving changed nothing on disk" "$(fingerprint "$W")" "$FP1"
ok "the list shows it: 4 changed files (a, b, c gone, d new), 1 staged, 1 new" "$(list "$W" | "$PY" -c 'import sys,json
s=json.load(sys.stdin)["value"]["states"][0]; print(s["name"], s["changed"], s["staged"], s["new_files"], s["branch"], s["commits_since"])')" "my first stage 4 1 1 main 0"
ok "  nothing else is filled" "$(list "$W" | "$PY" -c 'import sys,json
v=json.load(sys.stdin)["value"]; print(len(v["states"]), v["before"], v["max"])')" "1 None 4"
git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "commit 2"
printf 'b3\n' > "$W/b.txt"; git -C "$W" commit -qam "commit 3"; C3=$(head_of "$W")
printf 'e new\n' > "$W/e.txt"; printf 'SECRET=2\n' > "$W/.env"
FP3=$(fingerprint "$W")
ok "the list knows main gained 2 commits since" "$(list "$W" | "$PY" -c 'import sys,json
print(json.load(sys.stdin)["value"]["states"][0]["commits_since"])')" "2"
S1=$(oid_of "$W" 1)
OUT=$(back "$W" 1 "$S1")
ok "going back works" "$(printf '%s' "$OUT" | jqf value.ok)" "True"
ok "  HEAD is commit 1 again, on main" "$(head_of "$W") $(on_branch "$W")" "$C1 main"
ok "  every file, the staging and the status are exactly as saved" "$(fingerprint "$W")" "$FP1"
ok "  the file made since is gone" "$(there "$W/e.txt")" "gone"
ok "  ignored files are untouched" "$(cat "$W/.env") $(cat "$W/build/x")" "SECRET=2 x"
ok "  commits 2 and 3 are kept on a backup branch" "$(git -C "$W" for-each-ref --format='%(objectname)' 'refs/heads/gitswitch-before-state-*')" "$C3"
ok "  and the result says so, and that “Before going back” holds the rest" "$(printf '%s' "$OUT" | jqf value.detail)" "keeps them"
ok "  the state is still there to go back to again" "$( [ "$(oid_of "$W" 1)" = "$S1" ] && echo same || echo changed)" "same"
ok "  History never shows a state" "$(git -C "$W" log --oneline --branches --remotes --tags | grep -c 'GitSwitch state' || true)" "0"
OUT=$(back "$W" before "$(oid_of "$W" before)")
ok "“Before going back” returns all of it: commit 3, the new file, every byte" "$(printf '%s' "$OUT" | jqf value.ok) $(head_of "$W") $(fingerprint "$W")" "True $C3 $FP3"
OUT=$(back "$W" before "$(oid_of "$W" before)")
ok "  and again swaps back (one click each way)" "$(printf '%s' "$OUT" | jqf value.ok) $(head_of "$W") $(fingerprint "$W")" "True $C1 $FP1"

section "Commits made since are already pushed: ask, then a new branch or anyway"
fresh pu; W="$SB/pu"; C1=$(head_of "$W")
printf 'a2\n' > "$W/a.txt"; FPP=$(fingerprint "$W")
OUT=$(save "$W" 1 "before pushing")
git -C "$W" commit -qam "commit 2"; printf 'b2\n' > "$W/b.txt"; git -C "$W" commit -qam "commit 3"; C3=$(head_of "$W")
git -C "$W" push -q origin main 2>/dev/null
ok "the list counts the pushed commits since" "$(list "$W" | "$PY" -c 'import sys,json
s=json.load(sys.stdin)["value"]["states"][0]; print(s["commits_since"], s["pushed_since"], s["upstream"])')" "2 2 origin/main"
S1=$(oid_of "$W" 1)
OUT=$(back "$W" 1 "$S1")
ok "going back on main is refused, explained" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "state-drops-pushed"
ok "  in words" "$(printf '%s' "$OUT" | jqf value.refusal.message)" "already pushed"
ok "  and nothing changed" "$(head_of "$W") $(git -C "$W" status --porcelain | wc -l | tr -d ' ')" "$C3 0"
OUT=$(back "$W" 1 "$S1" '"rescue"')
ok "on a new branch it goes" "$(printf '%s' "$OUT" | jqf value.ok) $(on_branch "$W") $(head_of "$W")" "True rescue $C1"
ok "  with the files as saved" "$(fingerprint "$W")" "$FPP"
ok "  and main and origin/main are untouched" "$(git -C "$W" rev-parse main origin/main | tr '\n' ' ')" "$C3 $C3"
ok "  no backup branch was needed (main kept everything)" "$(git -C "$W" for-each-ref 'refs/heads/gitswitch-before-state-*' | wc -l | tr -d ' ')" "0"
OUT=$(back "$W" 1 "$S1" '"rescue"')
ok "a new branch that exists is refused" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "branch-exists"
OUT=$(back "$W" 1 "$S1" '"bad..name"')
ok "a name git can't take is refused" "$(printf '%s' "$OUT" | jqf error)" "branch"
OUT=$(back "$W" before "$(oid_of "$W" before)")
ok "(back on main at commit 3 through “Before going back”)" "$(printf '%s' "$OUT" | jqf value.ok) $(on_branch "$W") $(head_of "$W")" "True main $C3"
OUT=$(back "$W" 1 "$S1" null true)
ok "moving main back anyway works when asked" "$(printf '%s' "$OUT" | jqf value.ok) $(git -C "$W" rev-parse main) $(fingerprint "$W")" "True $C1 $FPP"
ok "  says main is behind its remote now, and never force-pushes" "$(printf '%s' "$OUT" | jqf value.detail)" "never does that"
ok "  the pushed commits are on a backup branch too" "$(git -C "$W" for-each-ref --format='%(objectname)' 'refs/heads/gitswitch-before-state-*')" "$C3"
ok "  and the remote has them still" "$(git -C "$W" rev-parse origin/main)" "$C3"

section "Files that must never be lost: ignored ones in the way, ignored only now"
fresh ig; W="$SB/ig"
printf 'kept log\n' > "$W/out.log"
OUT=$(save "$W" 1 "with a log")
printf '*.log\n' >> "$W/.gitignore"; git -C "$W" commit -qam "ignore logs"; HIG=$(head_of "$W")
printf 'my newer log\n' > "$W/out.log"
OUT=$(back "$W" 1 "$(oid_of "$W" 1)")
ok "an ignored file where the state has a file stops it" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "ignored-in-the-way"
ok "  naming it" "$(printf '%s' "$OUT" | jqf value.refusal.message)" "out.log"
ok "  and nothing changed: HEAD, the file" "$(head_of "$W") $(cat "$W/out.log")" "$HIG my newer log"
ok "  “Before going back” wasn't even written" "$( [ -z "$(oid_of "$W" before)" ] && echo none || echo written)" "none"
fresh ig2; W="$SB/ig2"
OUT=$(save "$W" 1 "clean")
printf 'notes.local\n' >> "$W/.gitignore"; git -C "$W" commit -qam "ignore notes"
printf 'private notes\n' > "$W/notes.local"
OUT=$(back "$W" 1 "$(oid_of "$W" 1)")
ok "a file ignored only now (not in the state's .gitignore) survives going back" "$(printf '%s' "$OUT" | jqf value.ok) $(cat "$W/notes.local")" "True private notes"
ok "  and the result says it was kept" "$(printf '%s' "$OUT" | jqf value.detail)" "notes.local"
fresh ig3; W="$SB/ig3"
printf 'plain\n' > "$W/cache"
OUT=$(save "$W" 1 "cache is a file")
rm "$W/cache"; printf 'cache/\n' >> "$W/.gitignore"; git -C "$W" commit -qam "cache is a folder now"
mkdir -p "$W/cache/deep"; printf 'precious\n' > "$W/cache/deep/data"
OUT=$(back "$W" 1 "$(oid_of "$W" 1)")
ok "an ignored folder where the state has a file stops it too" "$(printf '%s' "$OUT" | jqf value.refusal.code) $(cat "$W/cache/deep/data")" "ignored-in-the-way precious"

section "Where HEAD was: detached, a deleted branch, another worktree"
fresh hd; W="$SB/hd"; C1=$(head_of "$W")
git -C "$W" commit -q --allow-empty -m "commit 2"; C2=$(head_of "$W")
git -C "$W" checkout -q --detach "$C1"; printf 'detached edit\n' > "$W/a.txt"; FPD=$(fingerprint "$W")
OUT=$(save "$W" 2 "detached")
git -C "$W" checkout -q -f main
OUT=$(back "$W" 2 "$(oid_of "$W" 2)")
ok "a state saved detached comes back detached, main untouched" "$(printf '%s' "$OUT" | jqf value.ok) $(on_branch "$W") $(head_of "$W") $(git -C "$W" rev-parse main)" "True detached $C1 $C2"
ok "  with its files" "$(fingerprint "$W")" "$FPD"
git -C "$W" checkout -q -f main; git -C "$W" checkout -q -b topic "$C1"; printf 'topic edit\n' > "$W/b.txt"; FPT=$(fingerprint "$W")
OUT=$(save "$W" 3 "on topic")
git -C "$W" checkout -q -f main; git -C "$W" branch -q -D topic
ok "the list says the branch is gone" "$(list "$W" | "$PY" -c 'import sys,json
print(next(x for x in json.load(sys.stdin)["value"]["states"] if x["slot"]=="3")["branch_gone"])')" "True"
OUT=$(back "$W" 3 "$(oid_of "$W" 3)")
ok "going back recreates the deleted branch there" "$(printf '%s' "$OUT" | jqf value.ok) $(on_branch "$W") $(head_of "$W") $(fingerprint "$W")" "True topic $C1 $FPT"
git -C "$W" checkout -q -f main; git -C "$W" branch -q side "$C1"; git -C "$W" checkout -q side
OUT=$(save "$W" 4 "on side")
git -C "$W" checkout -q main
git -C "$W" worktree add -q "$SB/hd-wt" side 2>/dev/null
OUT=$(back "$W" 4 "$(oid_of "$W" 4)")
ok "a branch checked out in another worktree is refused, naming it" "$(printf '%s' "$OUT" | jqf value.refusal.code) $(printf '%s' "$OUT" | jqf value.refusal.message)" "branch-elsewhere"
ok "  each worktree has its own states (the other one sees none)" "$(list "$SB/hd-wt" | "$PY" -c 'import sys,json
v=json.load(sys.stdin)["value"]; print(len(v["states"]))')" "0"
git -C "$SB/hd-wt" gc -q --prune=now 2>/dev/null
ok "  and gc run from the other worktree keeps them" "$(git -C "$W" cat-file -e "$(oid_of "$W" 4)" && echo kept || echo pruned)" "kept"

section "Refused while it can't be whole, and the four slots"
fresh rf; W="$SB/rf"
git -C "$W" checkout -q -b other; printf 'other\n' > "$W/a.txt"; git -C "$W" commit -qam other
git -C "$W" checkout -q main; printf 'mine\n' > "$W/a.txt"; git -C "$W" commit -qam mine
git -C "$W" merge -q other >/dev/null 2>&1
OUT=$(save "$W" 1 "mid merge")
ok "saving during a merge is refused" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "operation-in-progress"
git -C "$W" merge --abort
OUT=$(save "$W" 1 "first")
OUT=$(save "$W" 1 "again")
ok "a taken slot is refused unless replaced" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "slot-taken"
OUT=$(save "$W" 1 "again" '"0000000"')
ok "replacing a state that isn't the one shown is refused" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "no-state"
OLD=$(oid_of "$W" 1); printf 'later\n' > "$W/b.txt"
OUT=$(save "$W" 1 "replaced" "\"$OLD\"")
ok "replacing the one shown works" "$(printf '%s' "$OUT" | jqf value.ok) $( [ "$(oid_of "$W" 1)" != "$OLD" ] && echo new)" "True new"
OUT=$(save "$W" 5 "five")
ok "there is no fifth slot" "$(printf '%s' "$OUT" | jqf error)" "not a state slot"
for n in 2 3 4; do save "$W" $n "s$n" >/dev/null; done
ok "four states fill the four slots" "$(list "$W" | "$PY" -c 'import sys,json
print(len(json.load(sys.stdin)["value"]["states"]))')" "4"
OUT=$(del "$W" 2 "0000000")
ok "deleting a state that isn't the one shown is refused" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "no-state"
OUT=$(del "$W" 2 "$(oid_of "$W" 2)")
ok "deleting the one shown empties the slot" "$(printf '%s' "$OUT" | jqf value.ok) $( [ -z "$(oid_of "$W" 2)" ] && echo empty || echo still)" "True empty"
ok "  and says what its commit was" "$(printf '%s' "$OUT" | jqf value.state.recovery)" "its commit was"
git -C "$W" checkout -q --orphan empty-branch; git -C "$W" rm -q -rf . 2>/dev/null
OUT=$(save "$W" 2 "nothing yet")
ok "a branch with no commit can't be saved" "$(printf '%s' "$OUT" | jqf value.refusal.code)" "unborn"

section "Each repository keeps as many states as its person sets"
fresh mx; W="$SB/mx"
ok "a repository keeps 4 to begin with, slot 1 free" "$(list "$W" | jqf value.max) $(list "$W" | jqf value.next_free)" "4 1"
for n in 1 2 3 4; do save "$W" $n "s$n" >/dev/null; done
ok "  with all four used no slot is free" "$(list "$W" | jqf value.next_free)" "None"
OUT=$(save "$W" 5 "five")
ok "  and a fifth is refused" "$(printf '%s' "$OUT" | jqf error)" "1 to 4"
setmax() { pinvoke states_set_max "{\"repoPath\":\"$1\",\"max\":$2}"; }
OUT=$(setmax "$W" 6)
ok "set to keep 6: the list says so, slot 5 is free" "$(printf '%s' "$OUT" | jqf value.max) $(printf '%s' "$OUT" | jqf value.next_free)" "6 5"
ok "  kept in the repository's own config" "$(git -C "$W" config --local --get gitswitch.stateSlots)" "6"
OUT=$(save "$W" 5 "five")
ok "  a fifth state saves" "$(printf '%s' "$OUT" | jqf value.ok)" "True"
OUT=$(setmax "$W" 4)
ok "lowering below a saved state is refused, naming it" "$(printf '%s' "$OUT" | jqf error)" "State 5 is saved above 4"
ok "  and nothing changed" "$(git -C "$W" config --local --get gitswitch.stateSlots) $( [ -n "$(oid_of "$W" 5)" ] && echo kept || echo lost)" "6 kept"
OUT=$(del "$W" 5 "$(oid_of "$W" 5)")
OUT=$(setmax "$W" 4)
ok "  once it is deleted, lowering works" "$(printf '%s' "$OUT" | jqf value.max)" "4"
OUT=$(setmax "$W" 0)
ok "a number out of range is refused" "$(printf '%s' "$OUT" | jqf error)" "from 1 to 99"
OUT=$(setmax "$W" 12)
for n in 5 6 7 8 9 10 11 12; do save "$W" $n "s$n" >/dev/null; done
ok "twelve states when the repository keeps twelve" "$(list "$W" | "$PY" -c 'import sys,json
v=json.load(sys.stdin)["value"]; print(len(v["states"]), v["max"], v["next_free"], [x["slot"] for x in v["states"]][-1])')" "12 12 None 12"
OUT=$(back "$W" 12 "$(oid_of "$W" 12)")
ok "  and going back to the twelfth works" "$(printf '%s' "$OUT" | jqf value.ok)" "True"

section "CRLF and binary files come back byte for byte"
fresh cr; W="$SB/cr"
printf 'crlf.txt text eol=crlf\n*.bin binary\n' > "$W/.gitattributes"
printf 'one\r\ntwo\r\n' > "$W/crlf.txt"; head -c 2048 /dev/urandom > "$W/blob.bin"
git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "line endings"
printf 'three\r\n' >> "$W/crlf.txt"; head -c 4096 /dev/urandom > "$W/blob.bin"; head -c 1024 /dev/urandom > "$W/new.bin"
FPC=$(fingerprint "$W")
OUT=$(save "$W" 1 "bytes")
git -C "$W" add -A >/dev/null; git -C "$W" commit -qm "more"; printf 'x' > "$W/blob.bin"
OUT=$(back "$W" 1 "$(oid_of "$W" 1)")
ok "every byte is as saved" "$(printf '%s' "$OUT" | jqf value.ok) $(fingerprint "$W")" "True $FPC"

section "Repositories inside: a submodule and a nested repository come back too, at any depth"
git init -q --bare "$SB/lib.git"; git clone -q "$SB/lib.git" "$SB/libwork" 2>/dev/null
printf 'lib1\n' > "$SB/libwork/l.txt"; git -C "$SB/libwork" add -A >/dev/null; git -C "$SB/libwork" commit -qm "lib 1"; git -C "$SB/libwork" push -q origin main 2>/dev/null
fresh su; W="$SB/su"
git -C "$W" -c protocol.file.allow=always submodule add -q "$SB/lib.git" lib 2>/dev/null
git -C "$W" commit -qam "add lib" >/dev/null; S1=$(head_of "$W")
git -C "$W/lib" checkout -q main 2>/dev/null
printf 'lib2\n' > "$W/lib/l.txt"; git -C "$W/lib" commit -qam "lib 2 (local)"; L2=$(head_of "$W/lib")
printf 'lib dirty\n' > "$W/lib/dirty.txt"; printf 'lib edit\n' >> "$W/lib/l.txt"
git init -q "$W/tools"; printf 't1\n' > "$W/tools/t.txt"; git -C "$W/tools" add -A >/dev/null; git -C "$W/tools" commit -qm "tools 1"; T1=$(head_of "$W/tools")
printf 'tools dirty\n' > "$W/tools/t.txt"
git init -q "$W/tools/deep"; printf 'd1\n' > "$W/tools/deep/d.txt"; git -C "$W/tools/deep" add -A >/dev/null; git -C "$W/tools/deep" commit -qm "deep 1"; D1=$(head_of "$W/tools/deep")
printf 'deep dirty\n' >> "$W/tools/deep/d.txt"
git init -q "$W/empty"
printf 'super edit\n' > "$W/a.txt"
FPS=$(fingerprint "$W"); FPL=$(fingerprint "$W/lib"); FPT=$(fingerprint "$W/tools"); FPD=$(fingerprint "$W/tools/deep")
OUT=$(save "$W" 1 "everything")
ok "the superproject saves with the repositories inside it" "$(printf '%s' "$OUT" | jqf value.ok) $(printf '%s' "$OUT" | jqf value.detail)" "True"
ok "  naming how many" "$(printf '%s' "$OUT" | jqf value.detail)" "repositories inside were saved"
ok "  the list shows each, at any depth" "$(list "$W" | "$PY" -c 'import sys,json
print(sorted(s["path"] for s in json.load(sys.stdin)["value"]["states"][0]["subs"]))')" "['empty', 'lib', 'tools', 'tools/deep']"
ok "  and nothing on disk changed, anywhere" "$(fingerprint "$W") $(fingerprint "$W/lib") $(fingerprint "$W/tools") $(fingerprint "$W/tools/deep")" "$FPS $FPL $FPT $FPD"
git -C "$W/lib" add -A >/dev/null; git -C "$W/lib" commit -qm "lib 3"; L3=$(head_of "$W/lib"); printf 'lib new\n' > "$W/lib/new.txt"
git -C "$W/tools" add -A >/dev/null; git -C "$W/tools" commit -qm "tools 2"; printf 'gone soon\n' > "$W/tools/extra.txt"
printf 'deep 2\n' > "$W/tools/deep/d.txt"; git -C "$W/tools/deep" commit -qam "deep 2"
printf 'super 2\n' > "$W/a.txt"; git -C "$W" add a.txt lib; git -C "$W" commit -qm "super 2"; S2=$(head_of "$W")
OUT=$(back "$W" 1 "$(oid_of "$W" 1)")
ok "going back brings all of them back" "$(printf '%s' "$OUT" | jqf value.ok)" "True"
ok "  the superproject: commit, files" "$(head_of "$W") $(fingerprint "$W")" "$S1 $FPS"
ok "  the submodule: its own commit, its own changes; its branch moved back" "$(head_of "$W/lib") $(on_branch "$W/lib") $(fingerprint "$W/lib")" "$L2 main $FPL"
ok "  and lib 3 is kept on a backup branch inside the submodule" "$(git -C "$W/lib" for-each-ref --format='%(objectname)' 'refs/heads/gitswitch-before-state-*')" "$L3"
ok "  the nested repository: its commit and its changes" "$(head_of "$W/tools") $(fingerprint "$W/tools")" "$T1 $FPT"
ok "  and the one nested inside that" "$(head_of "$W/tools/deep") $(fingerprint "$W/tools/deep")" "$D1 $FPD"
ok "  a repository with no commit yet is left as it is, said so" "$(printf '%s' "$OUT" | jqf value.detail)" "empty had no commit yet"
OUT=$(back "$W" before "$(oid_of "$W" before)")
ok "“Before going back” returns every one of them" "$(printf '%s' "$OUT" | jqf value.ok) $(head_of "$W") $(head_of "$W/lib") $(cat "$W/lib/new.txt") $(cat "$W/tools/extra.txt")" "True $S2 $L3 lib new gone soon"

verify_result
