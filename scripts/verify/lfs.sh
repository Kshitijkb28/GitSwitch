#!/bin/bash
# Git LFS suite: pointer detection and `git lfs pull`, proven against a local
# bare remote — git-lfs 3.x stores objects in <bare>/lfs/objects for a path
# remote, so no network and no account is needed.
set -u
source "$(dirname "$0")/lib.sh"
verify_init lfs "LFS Tester" "lfs@example.test"
IS_WIN=0; command -v cygpath >/dev/null 2>&1 && IS_WIN=1

if ! git lfs version >/dev/null 2>&1; then
  echo "git-lfs is not installed; skipping (install with: brew install git-lfs)"
  exit 0
fi

# --- an upstream with two LFS files and one ordinary file -------------------
git init -q --bare "$SB/remote.git"
git clone -q "$SB/remote.git" "$SB/author" 2>/dev/null
cd "$SB/author"
git lfs install --local >/dev/null 2>&1
git lfs track "*.bin" >/dev/null
head -c 4096 /dev/urandom > model.bin
mkdir -p "assets/big files"; head -c 2048 /dev/urandom > "assets/big files/tex ture.bin"
echo "plain" > README.md
git add -A >/dev/null; git commit -qm "lfs content"; git push -q origin main 2>/dev/null
ok "the bare remote holds the LFS objects" "$(find "$SB/remote.git/lfs/objects" -type f | wc -l | tr -d ' ')" "2"

# --- the case the feature exists for: a clone whose large files are stubs ----
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" work 2>/dev/null
STUB_BYTES=$(wc -c < "$SB/work/model.bin" | tr -d ' ')

section "Detecting pointer stubs"
OUT=$(probe status "$SB/work")
ok "status knows the repo uses LFS" "$(printf '%s' "$OUT" | jqf uses_lfs)" "True"
OUT=$(probe lfs_status "$SB/work")
ok "git-lfs is seen as installed" "$(printf '%s' "$OUT" | jqf installed)" "True"
ok "both tracked files are counted" "$(printf '%s' "$OUT" | jqf tracked)" "2"
ok "  and both are still pointers" "$(printf '%s' "$OUT" | jqf pointers)" "2"
ok "  a path with spaces is listed whole" "$(printf '%s' "$OUT" | jqf pointer_paths)" "assets/big files/tex ture.bin"
ok "  the summary says so in words" "$(printf '%s' "$OUT" | jqf summary)" "2 of 2 LFS files are still a pointer stub"
ok "  and notices the repo's LFS filters were never set up" "$(printf '%s' "$OUT" | jqf filters_configured)" "False"
ok "the stub really is a stub" "$([ "$STUB_BYTES" -lt 200 ] && echo tiny || echo "not a stub: $STUB_BYTES bytes")" "tiny"

section "Pulling the real content"
OUT=$(probe lfs_pull "$SB/work")
ok "the pull succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and reports how many arrived, measured not assumed" "$(printf '%s' "$OUT" | jqf headline)" "Downloaded 2 LFS files."
ok "  and says it had to set the filters up first" "$(printf '%s' "$OUT" | jqf detail)" "git lfs install --local"
ok "  no pointers remain" "$(printf '%s' "$OUT" | jqf lfs.pointers)" "0"
ok "  the file now holds its content" "$(wc -c < "$SB/work/model.bin" | tr -d ' ')" "4096"
ok "  including the one with spaces in its path" "$(wc -c < "$SB/work/assets/big files/tex ture.bin" | tr -d ' ')" "2048"
ok "  git agrees" "$(git -C "$SB/work" lfs ls-files | grep -c ' \* ' || true)" "2"

section "Pulling again is a harmless no-op"
OUT=$(probe lfs_pull "$SB/work")
ok "says everything is already here" "$(printf '%s' "$OUT" | jqf headline)" "already here"

section "A repository that doesn't use LFS"
git init -q "$SB/plain" && cd "$SB/plain" && echo x > f && git add -A >/dev/null && git commit -qm init
OUT=$(probe status "$SB/plain")
ok "status says it doesn't use LFS" "$(printf '%s' "$OUT" | jqf uses_lfs)" "False"
OUT=$(probe lfs_pull "$SB/plain")
ok "pull is refused by name, not by a git error" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-lfs"

section "A switch in a complete repository brings its large files along"
# Left to itself git downloads large files INSIDE the switch, through the
# smudge filter, on a 180s clock with nothing moving on screen. The app makes
# the switch write pointer stubs instantly and downloads afterwards on the
# large-file clock — and when everything was present before, everything is
# present after.
git -C "$SB/author" switch -q -c lfs-branch
head -c 3072 /dev/urandom > "$SB/author/branch-only.bin"
git -C "$SB/author" add -A >/dev/null; git -C "$SB/author" commit -qm "a large file only this branch has"; git -C "$SB/author" push -q origin lfs-branch 2>/dev/null
git -C "$SB/author" switch -q main
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" switcher 2>/dev/null
probe lfs_pull "$SB/switcher" >/dev/null
ok "the clone starts complete" "$(probe lfs_status "$SB/switcher" | jqf pointers)" "0"
# A local branch to switch to (the app refuses to guess at a remote-only name).
git -C "$SB/switcher" branch -q lfs-branch origin/lfs-branch
OUT=$(probe tree_branch_switch "$SB/switcher" "lfs-branch")
ok "the switch succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and says it downloaded the branch's large file" "$(printf '%s' "$OUT" | jqf detail)" "Downloaded 1 LFS file."
ok "  which really is content, not a stub" "$(wc -c < "$SB/switcher/branch-only.bin" | tr -d ' ')" "3072"
ok "  the whole repository is still complete" "$(probe lfs_status "$SB/switcher" | jqf pointers)" "0"
OUT=$(probe tree_branch_switch "$SB/switcher" "main")
ok "switching back needs no download and says nothing about one" "$(printf '%s' "$OUT" | jqf detail | grep -c "LFS" || true)" "0"

section "A switch in a deliberately partial repository stays partial, and says so"
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" partial 2>/dev/null
git -C "$SB/partial" branch -q lfs-branch origin/lfs-branch
probe lfs_pull_paths "$SB/partial" "model.bin" >/dev/null
ok "one of two files was downloaded on purpose" "$(probe lfs_status "$SB/partial" | jqf pointers)" "1"
OUT=$(probe tree_branch_switch "$SB/partial" "lfs-branch")
ok "the switch succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  the new large file is a stub, like the one left out before" "$(printf '%s' "$OUT" | jqf detail)" "2 LFS files here are a pointer stub"
ok "  no download was forced on a partial checkout" "$([ "$(wc -c < "$SB/partial/branch-only.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
ok "  and what was downloaded before is still here" "$(wc -c < "$SB/partial/model.bin" | tr -d ' ')" "4096"

section "git-lfs missing from this machine"
# git-lfs is a separate binary git finds via PATH. Every PATH directory that
# holds one is replaced by a shadow directory linking to everything else in
# it, so git (which may live in the same /usr/bin, as on Ubuntu) keeps working
# while `git lfs` becomes "not a git command" — the real missing-tool state.
# Each shadow becomes a PATH entry, so on Windows it has to be in the /c/… form:
# $SB is C:/…, and Git Bash splits a PATH entry at that colon (as sandbox.sh
# notes for its fake ssh). There `ln -s` makes copies — no symlinks without
# Developer Mode — and the copied git runs without its exec path, which only
# makes `git lfs` more surely "not a git command".
NOLFS="$SB/nolfs"
if command -v cygpath >/dev/null 2>&1; then NOLFS="$(cygpath -u "$NOLFS")"; fi
mkdir -p "$NOLFS"; STRIPPED=""
while IFS= read -r d; do
  [ -n "$d" ] || continue
  if [ -x "$d/git-lfs" ] || [ -x "$d/git-lfs.exe" ]; then
    shadow="$NOLFS/$(printf '%s' "$d" | tr '/:' '__')"; mkdir -p "$shadow"
    for f in "$d"/*; do b="$(basename "$f")"; case "$b" in git-lfs*) ;; *) ln -s "$f" "$shadow/$b" 2>/dev/null || true;; esac; done
    STRIPPED="$STRIPPED:$shadow"
  else
    STRIPPED="$STRIPPED:$d"
  fi
done <<<"$(printf '%s' "$PATH" | tr ':' '\n')"
STRIPPED="${STRIPPED#:}"
ok "the shadow PATH still finds git but not git-lfs" "$(PATH="$STRIPPED" command -v git >/dev/null && echo git-yes || echo git-no) $(PATH="$STRIPPED" command -v git-lfs >/dev/null && echo lfs-yes || echo lfs-no)" "git-yes lfs-no"
OUT=$(PATH="$STRIPPED" probe lfs_status "$SB/work")
ok "the status says git-lfs is not installed" "$(printf '%s' "$OUT" | jqf installed)" "False"
ok "  in words a person can act on" "$(printf '%s' "$OUT" | jqf summary)" "isn't installed"
OUT=$(PATH="$STRIPPED" probe lfs_pull "$SB/work")
# The hint names the platform's installer (lfs::install_hint).
case "$(uname -s)" in Darwin) LFS_HINT="brew install git-lfs";; Linux) LFS_HINT="apt install git-lfs";; *) LFS_HINT="git-lfs.com";; esac
ok "pull is refused with the install hint" "$(printf '%s' "$OUT" | jqf refusal.message)" "$LFS_HINT"
# With filter.lfs.required set, git status itself fails in this state, and its
# own words ("the remote end hung up unexpectedly") read like a network outage.
# git only runs the filter for a file whose stat data changed, so touch one to
# make the failure deterministic rather than dependent on index freshness.
touch "$SB/work/model.bin"
OUT=$(PATH="$STRIPPED" probe status "$SB/work")
ok "the page-level error names git-lfs, not the network" "$(printf '%s' "$OUT" | jqf error)" "git-lfs isn't installed"
ok "  and does not pass on the misleading 'remote end hung up'" "$(printf '%s' "$OUT" | grep -c "hung up" || true)" "0"

section "Pulling a repo that uses LFS"
# The author adds a NEW large file and pushes it.
cd "$SB/author"; head -c 6144 /dev/urandom > added.bin
git add -A >/dev/null; git commit -qm "add another large file"; git push -q origin main 2>/dev/null

# A clone whose LFS filters were never configured — the common case after a
# clone made before git-lfs was set up.
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" puller 2>/dev/null
ok "the new clone's filters are not configured" "$(git -C "$SB/puller" config --get filter.lfs.required || echo unset)" "unset"

# Pull WITHOUT the LFS option: git brings the commit, the content stays a stub.
OUT=$(probe pull "$SB/puller" "ff-only")
ok "a plain pull succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  but the new large file is still a pointer stub" "$([ "$(wc -c < "$SB/puller/added.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
ok "  and the result says so instead of leaving it silent" "$(printf '%s' "$OUT" | jqf detail)" "pointer stubs"

# Pull WITH the LFS option on a second clone: the content arrives.
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" puller2 2>/dev/null
OUT=$(probe pull "$SB/puller2" "ff-only,lfs")
ok "pulling with the LFS option downloads the content" "$([ "$(wc -c < "$SB/puller2/added.bin" | tr -d ' ')" -eq 6144 ] && echo content || echo stub)" "content"
ok "  the headline names how many arrived" "$(printf '%s' "$OUT" | jqf headline)" "Downloaded"
ok "  and says the filters had to be set up first" "$(printf '%s' "$OUT" | jqf detail)" "LFS filters weren't set up"
ok "  every stub in the checkout is resolved, not just the new one" "$(printf '%s' "$OUT" | jqf lfs.pointers)" "0"

section "Clone with the LFS option"
# The Clone page's checkbox: a fresh clone has no LFS filters, so without the
# option every large file is a pointer stub — and the result says so.
mkdir -p "$SB/clones"
OUT=$(probe clone "" "$SB/remote.git,$SB/clones,lfsclone,nosub,lfs")
ok "the clone succeeds" "$(printf '%s' "$OUT" | jqf path)" "lfsclone"
ok "  and downloaded the large files" "$(printf '%s' "$OUT" | jqf lfs.fetched)" "3"
ok "  none is a pointer stub" "$(printf '%s' "$OUT" | jqf lfs.pointers_left)" "0"
ok "  the fresh clone's filters had to be set up first" "$(printf '%s' "$OUT" | jqf lfs.configured_now)" "True"
ok "  the note says what arrived" "$(printf '%s' "$OUT" | jqf lfs.note)" "Downloaded 3 large files"
ok "  the file holds real bytes" "$(wc -c < "$SB/clones/lfsclone/model.bin" | tr -d ' ')" "4096"
# What the Clone page was shown meanwhile: the Large files phase opened at
# none of the stubs and closed at the measured count (a local download of a
# few KB is over before the first half-second read, so only the ends are certain).
TRACE=$(probe_trace)
ok "  the progress line opened the Large files phase at none of the stubs" "$(printf '%s' "$TRACE" | "$PY" -c 'import sys,json;p=[p for p in json.load(sys.stdin) if p["phase"]=="Large files" and p.get("done") is not None][0];print(p["done"],"of",p["total"])')" "0 of"
ok "  and the last thing recorded was Large files, every stub fetched — the measured count" "$(printf '%s' "$TRACE" | "$PY" -c 'import sys,json;p=json.load(sys.stdin)[-1];print(p["phase"],p["done"]==p["total"],p["percent"])')" "Large files True 100"
OUT=$(probe clone "" "$SB/remote.git,$SB/clones,nolfs,nosub")
ok "without the option the stubs stay" "$(printf '%s' "$OUT" | jqf lfs.pointers_left)" "3"
ok "  and the result points at the Changes page" "$(printf '%s' "$OUT" | jqf lfs.note)" "Changes"
ok "  the file is still a stub" "$([ "$(wc -c < "$SB/clones/nolfs/model.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
ok "  and without the option no large-files count was ever claimed" "$(probe_trace | "$PY" -c 'import sys,json;print([p for p in json.load(sys.stdin) if p["phase"]=="Large files" and p.get("done") is not None])')" "[]"
OUT=$(probe sparse_clone "" "$SB/remote.git,$SB/clones,sparse")
ok "a sparse clone has no LFS report yet (nothing is checked out)" "$(printf '%s' "$OUT" | jqf lfs)" "None"
OUT=$(probe sparse_set "$SB/clones/sparse" "assets,lfs")
ok "checking folders out with the option downloads their large files" "$(printf '%s' "$OUT" | jqf lfs.pointers_left)" "0"
ok "  the spaced path holds real bytes" "$(wc -c < "$SB/clones/sparse/assets/big files/tex ture.bin" | tr -d ' ')" "2048"
OUT=$(PATH="$STRIPPED" probe clone "" "$SB/remote.git,$SB/clones,nolfstool,nosub,lfs")
ok "without git-lfs the clone still succeeds" "$(printf '%s' "$OUT" | jqf path)" "nolfstool"
ok "  the report names the missing tool" "$(printf '%s' "$OUT" | jqf lfs.note)" "isn't installed"
ok "  installed is false" "$(printf '%s' "$OUT" | jqf lfs.installed)" "False"
OUT=$(probe lfs_available "$SB")
ok "the tool check answers without a repository" "$(printf '%s' "$OUT" | jqf installed)" "True"

# --- browsing and downloading a part of the repository ----------------------
# Read one field out of a JSON document on stdin with a python expression, for
# the list-shaped answers `jqf` (dict paths only) can't reach.
pysel() { "$PY" -c "
import sys,json
d=json.load(sys.stdin)
print($1)
"; }
# The frontend's own IPC, for an argument a comma-separated PROBE_ARGS can't carry.
invoke_json() {
  PROBE_OP=invoke PROBE_CMD="$1" PROBE_JSON="$2" "$BIN" probe --ignored --nocapture 2>/dev/null \
    | sed -n 's/^PROBE_OUT //p'
}

section "Browsing the large files folder by folder"
cd "$SB/author"
mkdir -p media/video media/audio
head -c 3072 /dev/urandom > media/video/clip.bin
head -c 1536 /dev/urandom > media/audio/tone.bin
head -c 700  /dev/urandom > "media/odd,name.bin"
git add -A >/dev/null; git commit -qm "more large files"; git push -q origin main 2>/dev/null
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" browse 2>/dev/null

OUT=$(probe lfs_files "$SB/browse")
ok "every tracked file is listed" "$(printf '%s' "$OUT" | jqf total)" "6"
ok "  none of them is here yet" "$(printf '%s' "$OUT" | jqf missing)" "6"
ok "  a stub still reports the real size it will take" \
  "$(printf '%s' "$OUT" | pysel "[f['size'] for f in d['files'] if f['path']=='model.bin'][0]")" "4096"
ok "  the total weight is the sum, not a guess" \
  "$(printf '%s' "$OUT" | jqf total_bytes)" "$((4096+2048+6144+3072+1536+700))"
ok "  a folder counts everything beneath it" \
  "$(printf '%s' "$OUT" | pysel "[x['files'] for x in d['folders'] if x['path']=='media'][0]")" "3"
ok "  a nested folder counts only its own" \
  "$(printf '%s' "$OUT" | pysel "[x['files'] for x in d['folders'] if x['path']=='media/video'][0]")" "1"
ok "  and carries what that folder would cost to download" \
  "$(printf '%s' "$OUT" | pysel "[x['missing_bytes'] for x in d['folders'] if x['path']=='media'][0]")" "$((3072+1536+700))"
ok "  a path with spaces survives whole" \
  "$(printf '%s' "$OUT" | pysel "[f['path'] for f in d['files'] if ' ' in f['path']][0]")" "assets/big files/tex ture.bin"
ok "  a file at the root belongs to no folder" \
  "$(printf '%s' "$OUT" | pysel "repr([f['dir'] for f in d['files'] if f['path']=='model.bin'][0])")" "''"
ok "  and the summary says it in words" "$(printf '%s' "$OUT" | jqf summary)" "6 of 6 LFS files are still a pointer stub"

section "Downloading one folder and nothing else"
OUT=$(probe lfs_pull_paths "$SB/browse" "media/video")
ok "it succeeds" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and names what arrived, with its weight" "$(printf '%s' "$OUT" | jqf headline)" "Downloaded 1 file (3.0 KB)."
ok "  the chosen file holds real content" "$(wc -c < "$SB/browse/media/video/clip.bin" | tr -d ' ')" "3072"
ok "  its neighbour in the next folder was left alone" \
  "$([ "$(wc -c < "$SB/browse/media/audio/tone.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
ok "  and so was the rest of the repository" \
  "$([ "$(wc -c < "$SB/browse/model.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
ok "  the counts that come back are measured, not assumed" "$(printf '%s' "$OUT" | jqf lfs.pointers)" "5"

section "Downloading one file, including a path git-lfs can't filter on"
# `--include` is comma-separated, so a comma in a path can only be fetched
# approximately — the checkout step is given the exact path, which is what makes
# the result precise. The UI sends real JSON, so the probe does too.
OUT=$(invoke_json changes_lfs_pull_paths "{\"repoPath\":\"$SB/browse\",\"paths\":[\"media/odd,name.bin\"]}")
ok "a path holding a comma still downloads" "$(printf '%s' "$OUT" | pysel "d['value']['ok']")" "True"
ok "  and holds its content" "$(wc -c < "$SB/browse/media/odd,name.bin" | tr -d ' ')" "700"
ok "  while its folder neighbour is still a stub" \
  "$([ "$(wc -c < "$SB/browse/media/audio/tone.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
OUT=$(invoke_json changes_lfs_pull_paths "{\"repoPath\":\"$SB/browse\",\"paths\":[\"assets/big files/tex ture.bin\"]}")
ok "a path holding spaces downloads too" "$(wc -c < "$SB/browse/assets/big files/tex ture.bin" | tr -d ' ')" "2048"

section "Asking again for what is already here"
OUT=$(probe lfs_pull_paths "$SB/browse" "media/video")
ok "says so instead of downloading again" "$(printf '%s' "$OUT" | jqf headline)" "already here"
ok "  and the file is untouched" "$(wc -c < "$SB/browse/media/video/clip.bin" | tr -d ' ')" "3072"

section "A selection that can't be honoured is refused by name"
OUT=$(probe lfs_pull_paths "$SB/browse" "no/such/folder")
ok "an unknown path is named, not guessed at" "$(printf '%s' "$OUT" | jqf refusal.code)" "unknown-path"
OUT=$(probe lfs_pull_paths "$SB/browse" "")
ok "an empty selection is refused" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-paths"
OUT=$(invoke_json changes_lfs_pull_paths "{\"repoPath\":\"$SB/browse\",\"paths\":[\"../escape.bin\"]}")
ok "a path leaving the repository never reaches git" "$(printf '%s' "$OUT" | pysel "d['value']['refusal']['code']")" "invalid-path"
OUT=$(invoke_json changes_lfs_pull_paths "{\"repoPath\":\"$SB/browse\",\"paths\":[\"--upload\"]}")
ok "and neither does one git would read as an option" "$(printf '%s' "$OUT" | pysel "d['value']['refusal']['code']")" "invalid-path"
OUT=$(probe lfs_pull_paths "$SB/plain" "anything")
ok "a repository without LFS is refused by name" "$(printf '%s' "$OUT" | jqf refusal.code)" "no-lfs"

section "A name holding a wildcard matches only itself"
# Every path given to git-lfs is a pattern. Written literally, `st*r.bin` also
# writes out `star.bin` — a file nobody selected — and `x[1].bin` fetches
# `x1.bin` INSTEAD of itself, leaving the chosen file a stub while the app
# blames the server.
cd "$SB/author"
head -c 111 /dev/urandom > "st*r.bin";  head -c 222 /dev/urandom > "star.bin"
head -c 333 /dev/urandom > "x[1].bin";  head -c 444 /dev/urandom > "x1.bin"
head -c 555 /dev/urandom > "q?r.bin";   head -c 666 /dev/urandom > "qZr.bin"
git add -A >/dev/null; git commit -qm "wildcard names" >/dev/null; git push -q origin main 2>/dev/null
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" globs 2>/dev/null
stub_or_content() { [ "$(wc -c < "$1" | tr -d ' ')" -lt 200 ] && echo stub || echo content; }

# NTFS holds no * or ? in a name (Git Bash writes a private-use character in
# its place), so on Windows those two files are other names and only [ ] is real.
if [ "$IS_WIN" = 1 ]; then
  echo "  SKIPPED: a file name holding * cannot exist on Windows, so it cannot be downloaded there"
else
  OUT=$(invoke_json changes_lfs_pull_paths "{\"repoPath\":\"$SB/globs\",\"paths\":[\"st*r.bin\"]}")
  ok "a name holding * downloads" "$(wc -c < "$SB/globs/st*r.bin" | tr -d ' ')" "111"
  ok "  and the file it would have globbed onto is untouched" "$(stub_or_content "$SB/globs/star.bin")" "stub"
fi
OUT=$(invoke_json changes_lfs_pull_paths "{\"repoPath\":\"$SB/globs\",\"paths\":[\"x[1].bin\"]}")
ok "a name holding [ ] downloads itself, not the file its class would match" \
  "$(wc -c < "$SB/globs/x[1].bin" | tr -d ' ')" "333"
ok "  and x1.bin is left alone" "$(stub_or_content "$SB/globs/x1.bin")" "stub"
if [ "$IS_WIN" = 1 ]; then
  echo "  SKIPPED: a file name holding ? cannot exist on Windows, so it cannot be downloaded there"
else
  OUT=$(invoke_json changes_lfs_pull_paths "{\"repoPath\":\"$SB/globs\",\"paths\":[\"q?r.bin\"]}")
  ok "a name holding ? downloads itself" "$(wc -c < "$SB/globs/q?r.bin" | tr -d ' ')" "555"
  ok "  and qZr.bin is left alone" "$(stub_or_content "$SB/globs/qZr.bin")" "stub"
fi
ok "  the progress file lives in the repository, not in shared temp" \
  "$(ls "$SB/globs/.git/" | grep -c gitswitch-lfs || true)" "0"

section "Downloading the rest"
OUT=$(probe lfs_pull "$SB/browse")
ok "everything else arrives" "$(printf '%s' "$OUT" | jqf lfs.pointers)" "0"
ok "  including the file in the folder that was skipped before" "$(wc -c < "$SB/browse/media/audio/tone.bin" | tr -d ' ')" "1536"
ok "  no progress file is left behind" "$(probe lfs_progress "$SB/browse")" "null"

section "A git-lfs error that leaves no file missing is a note, not a failure"
# git-lfs stopped after it wrote the last file (or failing on an object whose
# file is already here) exits with an error although nothing is missing. A
# git-lfs that does the whole pull and then exits 2 is exactly that case.
LFS_REAL="$(command -v git-lfs)"
# A PATH entry, so the /c/… form on Windows (see the shadow PATH above). git
# runs it, not the app — and Git for Windows runs a #!/bin/sh script on PATH.
STOPS="$SB/lfs-stops"
if command -v cygpath >/dev/null 2>&1; then STOPS="$(cygpath -u "$STOPS")"; fi
mkdir -p "$STOPS"
cat > "$STOPS/git-lfs" <<WRAP
#!/bin/sh
if [ "\$1" = "pull" ]; then
  "$LFS_REAL" "\$@"
  echo "error: git-lfs was stopped after writing the last file" >&2
  exit 2
fi
exec "$LFS_REAL" "\$@"
WRAP
chmod +x "$STOPS/git-lfs"
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" warn 2>/dev/null
git -C "$SB/warn" lfs install --local >/dev/null 2>&1
OUT=$(PATH="$STOPS:$PATH" probe lfs_pull "$SB/warn")
ok "(git-lfs itself exits with an error here)" "$(printf '%s' "$OUT" | jqf advice.git_said)" "stopped after writing the last file"
ok "a git-lfs error that leaves no stub behind is not a failure" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf lfs.pointers)" "True:0"
ok "  and the result still says git-lfs reported an error" "$(printf '%s' "$OUT" | jqf detail)" "git-lfs still exited with an error, but no file is missing"
# The same git-lfs where a stub is left: then it is a failure, as before.
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" warn-left 2>/dev/null
git -C "$SB/warn-left" lfs install --local >/dev/null 2>&1
cat > "$STOPS/git-lfs" <<WRAP
#!/bin/sh
if [ "\$1" = "pull" ]; then
  echo "error: git-lfs was stopped before writing anything" >&2
  exit 2
fi
exec "$LFS_REAL" "\$@"
WRAP
OUT=$(PATH="$STOPS:$PATH" probe lfs_pull "$SB/warn-left")
ok "  while one that leaves stubs is still a failure" "$(printf '%s' "$OUT" | jqf ok):$([ "$(printf '%s' "$OUT" | jqf lfs.pointers)" -gt 0 ] && echo stubs-left)" "False:stubs-left"

section "The large files of submodules: a clone and a Download bring them as the repository's own"
# A superproject with a large file of its own and a submodule with another.
git init -q --bare "$SB/slfs-lib.git"; git clone -q "$SB/slfs-lib.git" "$SB/slfs-lib-author" 2>/dev/null
( cd "$SB/slfs-lib-author" && git lfs install --local >/dev/null 2>&1 && git lfs track "*.bin" >/dev/null && head -c 2222 /dev/urandom > lib.bin && git add -A >/dev/null && git commit -qm "lib: a large file" && git push -q origin main 2>/dev/null )
git init -q --bare "$SB/slfs.git"; git clone -q "$SB/slfs.git" "$SB/slfs-author" 2>/dev/null
( cd "$SB/slfs-author" && git lfs install --local >/dev/null 2>&1 && git lfs track "*.bin" >/dev/null && head -c 1111 /dev/urandom > own.bin \
  && GIT_LFS_SKIP_SMUDGE=1 git submodule add -q "$SB/slfs-lib.git" lib 2>/dev/null && git add -A >/dev/null && git commit -qm "super: own large file, and lib" && git push -q origin main 2>/dev/null )
OUT=$(probe clone "" "$SB/slfs.git,$SB/clones,slfs-clone,submodules,lfs")
ok "a clone with submodules and large files brings the submodule's large files too" "$(wc -c < "$SB/clones/slfs-clone/lib/lib.bin" | tr -d ' ')" "2222"
ok "  and its result names them, by submodule" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([(s["path"], s["report"]["pointers_left"]) for s in json.load(sys.stdin)["submodule_lfs"]])')" "[('lib', 0)]"
OUT=$(probe clone "" "$SB/slfs.git,$SB/clones,slfs-nolfs,submodules")
ok "without the option the submodule's stubs are named, not left silent" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([(s["path"], s["report"]["pointers_left"] > 0) for s in json.load(sys.stdin)["submodule_lfs"]])')" "[('lib', True)]"
# A clone without its submodules, its own large files all here; then Download.
probe clone "" "$SB/slfs.git,$SB/clones,slfs-later,nosub,lfs" >/dev/null
OUT=$(invoke_json changes_submodule_update "{\"repoPath\":\"$SB/clones/slfs-later\",\"paths\":[\"lib\"]}")
ok "a submodule downloaded later brings its large files when the repository's own are all here" "$(wc -c < "$SB/clones/slfs-later/lib/lib.bin" | tr -d ' ')" "2222"
ok "  and says so" "$(printf '%s' "$OUT" | jqf value.detail)" "Git LFS in lib:"
# Under a push block the submodule's large files still arrive: git-lfs is set
# up before the block's hook goes in, and the block chains git-lfs's own.
probe clone "" "$SB/slfs.git,$SB/clones,slfs-blocked,nosub,lfs" >/dev/null
probe push_mode "$SB/clones/slfs-blocked" "guardrail" >/dev/null
OUT=$(invoke_json changes_submodule_update "{\"repoPath\":\"$SB/clones/slfs-blocked\",\"paths\":[\"lib\"]}")
ok "under a push block a downloaded submodule's large files still arrive, and it is blocked too" "$(wc -c < "$SB/clones/slfs-blocked/lib/lib.bin" | tr -d ' '):$(git -C "$SB/clones/slfs-blocked/lib" config --local --get gitswitch.pushBlockedBy)" "2222:superproject"
ok "  git-lfs's own push hook is kept, behind the block's" "$(ls "$(git -C "$SB/clones/slfs-blocked/lib" rev-parse --path-format=absolute --git-path hooks)" | grep -c '^pre-push.gitswitch-saved$')" "1"

section "Objects missing from the server"
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" work2 2>/dev/null
rm -rf "$SB/remote.git/lfs/objects"
OUT=$(probe lfs_pull "$SB/work2")
ok "the failure is explained as a server-side gap, not a local bug" "$(printf '%s' "$OUT" | tr -d '\n')" "server"
# Twelve LFS files exist by now: model.bin, the spaced one, added.bin, the three
# the browsing section added and the six wildcard-named ones.
ok "  and nothing pretends to have been downloaded" "$(printf '%s' "$OUT" | jqf lfs.pointers)" "12"

verify_result
