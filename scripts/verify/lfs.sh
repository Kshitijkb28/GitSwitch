#!/bin/bash
# Git LFS suite: pointer detection and `git lfs pull`, proven against a local
# bare remote — git-lfs 3.x stores objects in <bare>/lfs/objects for a path
# remote, so no network and no account is needed.
set -u
source "$(dirname "$0")/lib.sh"
verify_init lfs "LFS Tester" "lfs@example.test"

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

section "git-lfs missing from this machine"
# git lives in /usr/bin; git-lfs is a separate binary found via PATH.
LFS_DIR="$(dirname "$(command -v git-lfs)")"
STRIPPED="$(echo "$PATH" | tr ':' '\n' | grep -vx "$LFS_DIR" | paste -sd: -)"
OUT=$(PATH="$STRIPPED" probe lfs_status "$SB/work")
ok "the status says git-lfs is not installed" "$(printf '%s' "$OUT" | jqf installed)" "False"
ok "  in words a person can act on" "$(printf '%s' "$OUT" | jqf summary)" "isn't installed"
OUT=$(PATH="$STRIPPED" probe lfs_pull "$SB/work")
ok "pull is refused with the install hint" "$(printf '%s' "$OUT" | jqf refusal.message)" "brew install git-lfs"
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
OUT=$(probe clone "" "$SB/remote.git,$SB/clones,nolfs,nosub")
ok "without the option the stubs stay" "$(printf '%s' "$OUT" | jqf lfs.pointers_left)" "3"
ok "  and the result points at the Changes page" "$(printf '%s' "$OUT" | jqf lfs.note)" "Changes"
ok "  the file is still a stub" "$([ "$(wc -c < "$SB/clones/nolfs/model.bin" | tr -d ' ')" -lt 200 ] && echo stub || echo content)" "stub"
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

section "Objects missing from the server"
cd "$SB" && GIT_LFS_SKIP_SMUDGE=1 git clone -q "$SB/remote.git" work2 2>/dev/null
rm -rf "$SB/remote.git/lfs/objects"
OUT=$(probe lfs_pull "$SB/work2")
ok "the failure is explained as a server-side gap, not a local bug" "$(printf '%s' "$OUT" | tr -d '\n')" "server"
# Three LFS files exist by now (model.bin, the spaced one, added.bin).
ok "  and nothing pretends to have been downloaded" "$(printf '%s' "$OUT" | jqf lfs.pointers)" "3"

verify_result
