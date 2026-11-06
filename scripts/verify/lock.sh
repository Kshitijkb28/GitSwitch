#!/bin/bash
# Push lock suite: the privileged helper and the system-scope enforcement,
# measured with git the way a terminal would see it.
#
# Two modes:
#   test root (default; macOS/Linux): GITSWITCH_LOCK_ROOT (debug builds only)
#     re-roots every administrator-owned path into the sandbox and
#     GIT_CONFIG_SYSTEM points git's system scope at the re-rooted file. No
#     prompt, no privileges, the real /etc untouched — and the only mode where
#     the negative cases that need user-owned registry files can run.
#   LOCK_REAL=1 (CI runners): the REAL helper against the real system paths —
#     through `sudo -n` on Linux/macOS (the runners have passwordless sudo) or
#     directly on Windows (the runner is already elevated with UAC off). It
#     ends with uninstall and restores the system gitconfig it found. Do not
#     run it on a machine you care about without reading it first.
set -u
source "$(dirname "$0")/lib.sh"
verify_init lock "Lock Tester" "lock@example.test"

# This suite is about the system scope, so it must not skip it.
unset GIT_CONFIG_NOSYSTEM
# git matches `gitdir:` patterns against the REAL path of the .git directory
# (/var -> /private/var on macOS), so every repository path in a job must be
# canonical — the app canonicalises, and so does this suite.
SB="$(realdir "$SB")"
REAL="${LOCK_REAL:-}"
case "$(uname -s)" in
  Darwin) PLAT=macos;; Linux) PLAT=linux;; MINGW*|MSYS*|CYGWIN*) PLAT=windows;;
  *) echo "unsupported platform"; exit 1;;
esac
SUDO=""
if [ -z "$REAL" ]; then
  if [ "$PLAT" = windows ]; then echo "test-root mode needs an unprivileged process; on Windows run with LOCK_REAL=1"; exit 0; fi
  LOCK_ROOT="$SB/root"; mkdir -p "$LOCK_ROOT/etc"
  export GITSWITCH_LOCK_ROOT="$LOCK_ROOT"
  export GIT_CONFIG_SYSTEM="$LOCK_ROOT/etc/gitconfig"
  export GITSWITCH_ELEVATE=direct
else
  unset GITSWITCH_LOCK_ROOT GIT_CONFIG_SYSTEM
  if [ "$PLAT" = windows ]; then
    export GITSWITCH_ELEVATE=direct            # the runner is elevated already
  else
    sudo -n true 2>/dev/null || { echo "LOCK_REAL=1 needs passwordless sudo (CI runners have it)"; exit 1; }
    SUDO="sudo -n"; export GITSWITCH_ELEVATE=sudo
  fi
  # The lock covers Apple's git on macOS (its system scope is /etc/gitconfig);
  # a Homebrew git reads /opt/homebrew/etc/gitconfig and is disclosed as not
  # covered. CI runners put Homebrew first on PATH, so the terminal checks
  # here use the git the lock is for.
  if [ "$PLAT" = macos ]; then export PATH="/usr/bin:$PATH"; fi
fi

# Every managed path comes from the app's own layout, which honours the test
# root — nothing here hard-codes /etc or C:/ProgramData.
mkdir -p "$SB/probe-dir"
ST=$(probe lock_helper_status "$SB/probe-dir")
HELPER=$(printf '%s' "$ST" | jqf helper_path)
SYSCFG=$(printf '%s' "$ST" | jqf system_gitconfig)
REG=$(printf '%s' "$ST" | jqf registry_dir)
REMOTE_HELPER=$(printf '%s' "$ST" | jqf remote_helper_path)
AUDIT=$(printf '%s' "$ST" | jqf audit_log)
[ -n "$HELPER" ] && [ "$HELPER" != None ] || { echo "the app reports no lock layout on this platform"; exit 1; }
echo "mode: $([ -n "$REAL" ] && echo REAL || echo test-root)  platform: $PLAT  registry: $REG  system gitconfig: $SYSCFG  git: $(command -v git)"
export PATH="$(dirname "$REMOTE_HELPER"):$PATH"      # where the remote helper lands (unix); git's exec path on Windows
JOB_SYSCFG=$([ -n "$REAL" ] && printf '%s' "$SYSCFG" || printf '/etc/gitconfig')

# Real mode: keep whatever system gitconfig the machine had, to restore at the end.
SYSCFG_ORIG=""
if [ -n "$REAL" ] && [ -e "$SYSCFG" ]; then SYSCFG_ORIG="$SB/syscfg.orig"; cat "$SYSCFG" > "$SYSCFG_ORIG"; fi

# Privileged file helpers: sudo on unix in real mode, direct otherwise.
sys_write() { if [ -n "$SUDO" ]; then $SUDO tee "$1" >/dev/null; else cat > "$1"; fi; }   # content on stdin
sys_rm() { $SUDO rm -rf "$@"; }
sys_cp() { $SUDO cp "$1" "$2"; $SUDO chmod 755 "$2" 2>/dev/null || true; }
owner_mode() { # <path> -> "owner mode" (unix only)
  case "$PLAT" in macos) stat -f '%Su %Lp' "$1";; linux) stat -c '%U %a' "$1";; *) echo "n/a";; esac; }

git init -q --bare "$SB/remote.git"
git clone -q "$SB/remote.git" "$SB/work" 2>/dev/null
cd "$SB/work"; echo a > a.txt; git add -A >/dev/null; git commit -qm init; git push -q -u origin main 2>/dev/null
REMOTE_URL="$(git -C "$SB/work" remote get-url origin)"

# job <nonce> <ops-json> -> path of the written job file
job() { mkdir -p "$SB/jobs"; local f="$SB/jobs/$1.json"
  printf '{"schema":1,"nonce":"%s","app_version":"0.1.0","platform":"%s","system_gitconfig":"%s","requested_by_uid":%s,"ops":%s}' "$1" "$PLAT" "$JOB_SYSCFG" "$(id -u)" "$2" > "$f"; echo "$f"; }
sha() { sha256file "$1"; }
# run <helper-binary> <job> [--bootstrap] -> sets OUT (stdout), RC, and writes stderr to $SB/err.txt
run() { local bin="$1" j="$2" flag="${3:-}"
  if [ -n "$flag" ]; then OUT=$($SUDO "$bin" "$flag" "$j" --sha256 "$(sha "$j")" 2>"$SB/err.txt"); else OUT=$($SUDO "$bin" "$j" --sha256 "$(sha "$j")" 2>"$SB/err.txt"); fi; RC=$?
  # In real mode a non-zero exit is a finding in itself: say what the helper
  # said, so a CI log is enough to diagnose it (test-root mode provokes
  # refusals on purpose and would only be noisier).
  if [ "$RC" != 0 ] && [ -n "$REAL" ]; then
    printf '     helper rc=%s stderr: %s\n' "$RC" "$(tr '\n' ' ' < "$SB/err.txt" | head -c 700)"
    printf '     helper stdout: %s\n' "$(printf '%s' "$OUT" | head -c 700)"
  fi; }
LOCK_OPS="[{\"op\":\"bootstrap\"},{\"op\":\"lock\",\"repo\":\"$SB/work\",\"gitdir\":\"$SB/work/.git\",\"block_prefixes\":[\"git@github.com:\",\"https://github.com/\",\"$REMOTE_URL\"],\"label\":\"work\"}]"
lower() { tr 'A-Z' 'a-z'; }

section "Bootstrap installs the helper into the admin-owned location and applies the lock"
J=$(job n1 "$LOCK_OPS")
run "$HELPER_BUNDLED" "$J" --bootstrap
ok "the bundled helper exits 0" "$RC" "0"
ok "  the result says ok" "$(printf '%s' "$OUT" | jqf ok)" "True"
ok "  and names what it did" "$(printf '%s' "$OUT" | jqf message)" "Locked 1 repository."
ok "  the result echoes the job's nonce" "$(printf '%s' "$OUT" | jqf nonce)" "n1"
ok "an installed copy now exists" "$([ -x "$HELPER" ] && echo yes || echo no)" "yes"
ok "  with the same bytes as the bundle" "$(cmp -s "$HELPER" "$HELPER_BUNDLED" && echo same || echo differs)" "same"
ok "  and --version answers from it" "$("$HELPER" --version)" "gitswitch-lock-helper"
ok "the registry records the lock" "$(jqf 'locks' < "$REG/locks.json" | lower)" "\"repo\": \"$(printf '%s' "$SB/work" | lower)\""
ok "  and the helper's checksum" "$(jqf 'helper.sha256' < "$REG/locks.json")" "$(sha "$HELPER")"
ok "a result file appears next to the job" "$([ -f "$J.result.json" ] && jqf ok < "$J.result.json")" "True"
ok "the audit log has the bootstrap and the job" "$(grep -c 'nonce=n1' "$AUDIT")" "2"
if [ "$PLAT" = windows ]; then
  ok "the remote helper is installed under git's name (a copy, in git's exec path)" "$(cmp -s "$REMOTE_HELPER" "$HELPER" && echo same || echo differs)" "same"
else
  ok "the remote helper is installed under git's name" "$(readlink "$REMOTE_HELPER")" "$HELPER"
fi
if [ -n "$REAL" ]; then
  case "$PLAT" in
    macos) ok "REAL: the registry is root:wheel 0755" "$(owner_mode "$REG")" "root 755"
           ok "REAL: the registry file is root-owned 0644" "$(owner_mode "$REG/locks.json")" "root 644"
           ok "REAL: the helper is root-owned 0755" "$(owner_mode "$HELPER")" "root 755";;
    linux) ok "REAL: the registry is root 755" "$(owner_mode "$REG")" "root 755"
           ok "REAL: the registry file is root 644" "$(owner_mode "$REG/locks.json")" "root 644"
           ok "REAL: the helper is root 755" "$(owner_mode "$HELPER")" "root 755"
           # The policy is written only into an administrator-only directory.
           # GitHub's runner image makes /usr/share/polkit-1/actions writable
           # by the runner user, so there the helper must refuse and say why.
           if [ -f /usr/share/polkit-1/actions/com.gitswitch.lock-helper.policy ]; then
             ok "REAL: the polkit policy is installed" "$(cat /usr/share/polkit-1/actions/com.gitswitch.lock-helper.policy)" "auth_admin"
           else
             ok "REAL: no policy written, and the result says why (no polkit, or a directory that is not administrator-only)" "$(printf '%s' "$OUT" | jqf changed)" "not installed"
           fi;;
    windows) ACL=$(icacls "$(cygpath -w "$REG")" 2>/dev/null | tr -d '\r')
           ok "REAL: Administrators have full control of the registry" "$ACL" "Administrators:(OI)(CI)(F)"
           ok "REAL: Users can only read it" "$ACL" "Users:(OI)(CI)(RX)"
           ok "REAL: no Users write access anywhere on it" "$(printf '%s' "$ACL" | grep -c 'Users:.*(W)\|Users:.*(M)\|Users:.*(F)' || true)" "0";;
  esac
fi

section "The system scope now carries the rule"
ok "the marker block is in the system gitconfig" "$(grep -c 'GitSwitch push lock' "$SYSCFG")" "2"
ok "  and includes the registry's include file" "$(grep 'path = ' "$SYSCFG")" "$REG/locks.gitconfig"
ok "the include file targets exactly this repo, case-insensitively" "$(lower < "$REG/locks.gitconfig")" "[includeif \"gitdir/i:$(printf '%s' "$SB/work" | lower)/\"]"
SCOPED=$(git -C "$SB/work" config --show-scope --show-origin --get-all 'url.gitswitch-push-blocked://.pushInsteadOf' | head -1)
ok "git itself reports the rewrite at SYSTEM scope" "$SCOPED" "system"
ok "  coming from the stanza file" "$SCOPED" "locks.d/"
ok "gitswitch.locked is true, from the system scope" "$(git -C "$SB/work" config --show-scope --get gitswitch.locked)" "system	true"
ok "the push URL is rewritten" "$(git -C "$SB/work" remote get-url --push origin)" "gitswitch-push-blocked://"
ok "  while the fetch URL is untouched" "$(git -C "$SB/work" remote get-url origin)" "$REMOTE_URL"
ok "a sibling repo is NOT affected" "$(git init -q "$SB/other" && git -C "$SB/other" config --get gitswitch.locked || echo unset)" "unset"

section "Pushes are refused in a plain terminal"
echo b >> "$SB/work/a.txt"; git -C "$SB/work" commit -qam "second"
P=$(git -C "$SB/work" push origin main 2>&1; echo "rc=$?")
ok "git push fails" "$P" "rc=128"
ok "  with our words, not git's" "$P" "GitSwitch: push refused."
ok "  naming the repository" "$(printf '%s' "$P" | lower)" "repository: $(printf '%s' "$SB/work" | lower)"
ok "  telling agents not to work around it" "$P" "Do not work around this"
ok "  with the machine-readable trailer" "$P" "gitswitch: lock=on policy=ask-owner"
ok "  and never printing a git command to undo it" "$(printf '%s' "$P" | grep -c 'git config' || true)" "0"
P2=$(git -C "$SB/work" push --no-verify origin main 2>&1; echo "rc=$?")
ok "--no-verify changes nothing (this is not a hook)" "$P2" "GitSwitch: push refused."
P3=$(git -C "$SB/work" push "$REMOTE_URL" main 2>&1; echo "rc=$?")
ok "a literal URL matching a pinned prefix is refused too" "$P3" "GitSwitch: push refused."
ok "the remote did not move" "$(git -C "$SB/remote.git" log -1 --format=%s main)" "init"
ok "fetch still works" "$(git -C "$SB/work" fetch origin 2>&1; echo "rc=$?")" "rc=0"

section "The helper refuses what it should"
J=$(job n2 "[{\"op\":\"lock\",\"repo\":\"$SB/work\",\"gitdir\":\"$SB/work/.git\",\"block_prefixes\":[\"x:\"],\"label\":\"w\"}]")
S=$(sha "$J"); printf ' ' >> "$J"
OUT=$($SUDO "$HELPER" "$J" --sha256 "$S" 2>"$SB/err.txt"); RC=$?
ok "a job changed after approval is refused (exit 3)" "$RC" "3"
ok "  saying why" "$(cat "$SB/err.txt")" "does not match the checksum"
J=$(job n3 "[{\"op\":\"lock\",\"repo\":\"relative/path\",\"gitdir\":\"/x/.git\",\"block_prefixes\":[\"x:\"],\"label\":\"w\"}]")
run "$HELPER" "$J"; ok "a relative repo path is refused (exit 3)" "$RC" "3"
J=$(job n4 "[{\"op\":\"lock\",\"repo\":\"$SB/work\",\"gitdir\":\"$SB/work/.git\",\"block_prefixes\":[\"a\\\"b\"],\"label\":\"w\"}]")
run "$HELPER" "$J"; ok "a prefix with a quote in it is refused" "$RC" "3"
J=$(job n5 "[{\"op\":\"lock\",\"repo\":\"$SB/work\",\"gitdir\":\"$SB/work/.git\",\"block_prefixes\":[\"x:\"],\"label\":\"w\"},{\"op\":\"unlock\",\"repo\":\"$SB/other\"}]")
run "$HELPER" "$J"; ok "lock and unlock in one job are refused" "$RC" "3"
ok "  because the prompt could not show which" "$(cat "$SB/err.txt")" "never both"
mkdir -p "$SB/jobs"; printf '{"schema":1,"nonce":"n6","app_version":"0","platform":"%s","system_gitconfig":"%s","ops":[{"op":"repair-system"}],"surprise":true}' "$PLAT" "$JOB_SYSCFG" > "$SB/jobs/n6.json"
run "$HELPER" "$SB/jobs/n6.json"; ok "an unknown field is a usage error (exit 2)" "$RC" "2"
printf '{"schema":1,"nonce":"n7","app_version":"0","platform":"%s","system_gitconfig":"%s/.gitconfig","ops":[{"op":"repair-system"}]}' "$PLAT" "$HOME" > "$SB/jobs/n7.json"
run "$HELPER" "$SB/jobs/n7.json"; ok "a system gitconfig outside the allowlist is refused" "$RC" "3"
OUT=$($SUDO "$HELPER" 2>"$SB/err.txt"); RC=$?; ok "no arguments is a usage error" "$RC" "2"
OUT=$($SUDO "$HELPER" "$SB/jobs/n6.json" --sha256 nothex 2>"$SB/err.txt"); RC=$?; ok "a malformed checksum never reaches the filesystem" "$RC" "2"
J=$(job n8 '[{"op":"repair-system"}]')
run "$HELPER_BUNDLED" "$J"; ok "the bundled (user-writable) copy may not apply jobs" "$RC" "3"
ok "  it points at the installed one" "$(cat "$SB/err.txt")" "only the installed helper"
run "$HELPER_BUNDLED" "$J" --bootstrap; ok "a second bootstrap over a valid install is refused" "$RC" "3"
ok "  and says so" "$(cat "$SB/err.txt")" "already installed"
if [ -z "$REAL" ]; then
  # Planted links need user-owned registry files: test-root mode only. In real
  # use nobody but the administrator can create anything inside the registry.
  mv "$REG/locks.d" "$SB/stolen"; ln -s "$SB/stolen" "$REG/locks.d"
  run "$HELPER" "$J"; ok "a symlinked registry directory is refused, not followed" "$RC" "3"
  rm "$REG/locks.d"; mv "$SB/stolen" "$REG/locks.d"
  mv "$REG/locks.json" "$SB/locks.json"; ln -s "$SB/locks.json" "$REG/locks.json"
  run "$HELPER" "$J"; ok "a symlinked registry file is refused" "$RC" "3"
  rm "$REG/locks.json"; mv "$SB/locks.json" "$REG/locks.json"
fi
run "$HELPER" "$J"; ok "with the registry intact, repair is a clean no-op" "$(printf '%s' "$OUT" | jqf message)" "Nothing needed doing."
ok "  and exits 0" "$RC" "0"

section "Idempotence and a second lock"
J=$(job n9 "$LOCK_OPS")
run "$HELPER" "$J"; ok "re-locking the same repo changes nothing" "$(printf '%s' "$OUT" | jqf changed)" "already locked; unchanged"
ok "  the stanza still exists exactly once" "$(find "$REG/locks.d" -type f | wc -l | tr -d ' ')" "1"
mkdir -p "$SB/with space"; git clone -q "$SB/remote.git" "$SB/with space/second" 2>/dev/null
J=$(job n10 "[{\"op\":\"lock\",\"repo\":\"$SB/with space/second\",\"gitdir\":\"$SB/with space/second/.git\",\"block_prefixes\":[\"$REMOTE_URL\"],\"label\":\"second\"}]")
run "$HELPER" "$J"; ok "a repo with a space in its path locks" "$RC" "0"
ok "  git sees its rule at system scope" "$(git -C "$SB/with space/second" config --show-scope --get gitswitch.locked)" "system	true"
ok "  the first repo is still locked" "$(git -C "$SB/work" config --get gitswitch.locked)" "true"
git -C "$SB/work" worktree add -q "$SB/wt" -b wt 2>/dev/null
ok "a linked worktree of a locked repo is locked" "$(git -C "$SB/wt" config --get gitswitch.locked)" "true"
mkdir -p "$SB/work/nested"; git init -q "$SB/work/nested"
ok "a repo nested inside a locked work tree is locked too (disclosed side effect)" "$(git -C "$SB/work/nested" config --get gitswitch.locked)" "true"

section "The documented bypasses exist — the disclosure must be true"
BY=$(GIT_CONFIG_NOSYSTEM=1 git -C "$SB/work" push origin main 2>&1; echo "rc=$?")
ok "GIT_CONFIG_NOSYSTEM=1 gets past the system rule (no password involved)" "$BY" "rc=0"
ok "  and the remote moved" "$(git -C "$SB/remote.git" log -1 --format=%s main)" "second"
echo c >> "$SB/work/a.txt"; git -C "$SB/work" commit -qam "third"
BY2=$(git -C "$SB/work" -c "remote.origin.pushurl=$REMOTE_URL" push origin main 2>&1; echo "rc=$?")
ok "an explicit pushurl is exempt from pushInsteadOf, as git documents" "$BY2" "rc=0"
echo d >> "$SB/work/a.txt"; git -C "$SB/work" commit -qam "fourth"
NULLCFG=$([ "$PLAT" = windows ] && echo NUL || echo /dev/null)
BY3=$(GIT_CONFIG_SYSTEM="$NULLCFG" git -C "$SB/work" push origin main 2>&1; echo "rc=$?")
ok "GIT_CONFIG_SYSTEM=$NULLCFG does the same" "$BY3" "rc=0"
echo e >> "$SB/work/a.txt"; git -C "$SB/work" commit -qam "fifth"
STILL=$(git -C "$SB/work" push origin main 2>&1; echo "rc=$?")
ok "a plain push is still refused afterwards" "$STILL" "GitSwitch: push refused."

section "Foreign content in the system gitconfig survives byte for byte"
J=$(job n11 "[{\"op\":\"unlock\",\"repo\":\"$SB/work\"},{\"op\":\"unlock\",\"repo\":\"$SB/with space/second\"}]")
run "$HELPER" "$J"; ok "unlocking both repos" "$(printf '%s' "$OUT" | jqf message)" "Unlocked 2 repositories."
ok "  removes the marker block (and the file itself when it held nothing else)" "$(grep -c 'GitSwitch push lock' "$SYSCFG" 2>/dev/null || echo 0)" "0"
printf '[core]\n\tautocrlf = input\n# keep me\n[alias]\n\tco = checkout\n' | sys_write "$SYSCFG"
cat "$SYSCFG" > "$SB/foreign.before"
J=$(job n12 "$LOCK_OPS"); run "$HELPER" "$J"
ok "locking appends our block after the foreign content" "$(head -c 20 "$SYSCFG" | tr '\n' ' ')" "[core]"
ok "  the alias survives" "$(git -C "$SB/work" config --get alias.co)" "checkout"
ok "  a backup identical to the foreign file was taken first" "$(for b in "$REG"/backups/gitconfig.*; do cmp -s "$b" "$SB/foreign.before" && echo found; done)" "found"
J=$(job n13 "[{\"op\":\"unlock\",\"repo\":\"$SB/work\"}]"); run "$HELPER" "$J"
ok "unlocking gives the foreign file back byte for byte" "$(cmp -s "$SYSCFG" "$SB/foreign.before" && echo identical || echo differs)" "identical"
ok "  no include file, no stanza" "$(find "$REG/locks.d" -type f | wc -l | tr -d ' '):$([ -e "$REG/locks.gitconfig" ] && echo include || echo none)" "0:none"
P=$(git -C "$SB/work" push origin main 2>&1; echo "rc=$?")
ok "pushes work again" "$P" "rc=0"
ok "  the remote has the commit" "$(git -C "$SB/remote.git" log -1 --format=%s main)" "fifth"

section "Uninstall removes every trace"
J=$(job n14 "$LOCK_OPS"); run "$HELPER" "$J"
J=$(job n15 '[{"op":"uninstall-all"}]'); run "$HELPER" "$J"
ok "uninstall exits 0" "$RC" "0"
# On Windows the helper lives inside the registry and, being the running
# process, is renamed aside and deleted at reboot; everything else is gone.
reg_left() { if [ ! -e "$REG" ]; then echo gone; elif [ "$PLAT" = windows ] && [ "$(find "$REG" -type f ! -name '*.old' | wc -l | tr -d ' ')" = 0 ]; then echo gone; else echo present; fi; }
ok "  the registry directory is gone" "$(reg_left)" "gone"
ok "  the remote helper is gone" "$([ -e "$REMOTE_HELPER" ] && echo present || echo gone)" "gone"
ok "  the helper removed itself" "$([ -e "$HELPER" ] && echo present || echo gone)" "gone"
if [ "$PLAT" = windows ]; then
  ok "  the running exe was moved aside for deletion at reboot" "$(ls "$(dirname "$HELPER")"/*.old 2>/dev/null | wc -l | tr -d ' ')" "1"
fi
ok "  the foreign system gitconfig is intact" "$(cmp -s "$SYSCFG" "$SB/foreign.before" && echo identical || echo differs)" "identical"
ok "  git no longer sees a lock" "$(git -C "$SB/work" config --get gitswitch.locked || echo unset)" "unset"

# ---------------------------------------------------------------------------
# From here on the APP drives the helper, with GITSWITCH_ELEVATE (debug builds
# only) standing in for the OS prompt: `direct` runs the helper as this user
# against the test root, `sudo` runs it as real root on a CI runner.
# ---------------------------------------------------------------------------
git clone -q "$SB/remote.git" "$SB/app" 2>/dev/null
APP="$SB/app"

section "Locking from the app"
OUT=$(probe push_mode "$APP" "locked")
ok "the outcome is applied" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
ok "  and the helper was installed on the way (it was uninstalled above)" "$([ -x "$HELPER" ] && echo installed || echo missing)" "installed"
ok "  the state says locked" "$(printf '%s' "$OUT" | jqf state.lock.locked)" "True"
ok "  git's own measurement confirms the system-scope rewrite" "$(printf '%s' "$OUT" | jqf state.lock.system_rewrite_measured)" "True"
# The remote helper lives in a PATH directory (/usr/local/bin). On CI runners
# that directory belongs to the runner user, and the app must say so: that is
# the disclosed "remote-helper-unprotected-dir" drift, and the only drift allowed.
EXPECT_DRIFT="[]"
if [ -n "$REAL" ] && [ "$PLAT" != windows ]; then
  # Same rule as the app: protected means root-owned with no group/other write bit.
  read -r rh_owner rh_mode <<<"$(owner_mode "$(dirname "$REMOTE_HELPER")")"
  if [ "$rh_owner" != root ] || [ $(( 8#$rh_mode & 8#22 )) -ne 0 ]; then EXPECT_DRIFT='["remote-helper-unprotected-dir"]'; fi
fi
ok "  no layer has drifted (beyond a disclosed unprotected remote-helper directory)" "$(printf '%s' "$OUT" | jqf state.lock.drift)" "$EXPECT_DRIFT"
ok "  the mirrors are in place" "$(printf '%s' "$OUT" | jqf state.lock.mirrors)" "ok"
ok "  the reason names the administrator password" "$(printf '%s' "$OUT" | jqf state.reason)" "administrator password"
ok "  the pre-push hook is installed" "$(head -3 "$APP/.git/hooks/pre-push" | tr '\n' ' ')" "GitSwitch push guard"
ok "  the local flag mirrors the lock" "$(git -C "$APP" config --local --get gitswitch.pushBlocked)" "true"
ok "  the caveats name the real bypass" "$(printf '%s' "$OUT" | jqf state.lock.caveats)" "GIT_CONFIG_NOSYSTEM=1"
ok "  and never point at GitHub (the user chose a local lock)" "$(printf '%s' "$OUT" | jqf state.lock.caveats | grep -c GitHub || true)" "0"
ok "  the event log records the lock" "$(printf '%s' "$OUT" | jqf state.lock.recent_events)" "\"code\": \"locked\""

section "The app refuses before running git, and the terminal is refused by git"
echo x >> "$APP/a.txt"; git -C "$APP" commit -qam "x"
OUT=$(probe push "$APP" "")
ok "the app's refusal code is push-locked" "$(printf '%s' "$OUT" | jqf refusal.code)" "push-locked"
P=$(git -C "$APP" push origin main 2>&1; echo "rc=$?")
ok "a terminal push is refused with the lock's words" "$P" "GitSwitch: push refused."

section "An explicit pushurl: the rewrite is exempt, the hook is not"
git -C "$APP" config remote.origin.pushurl "$REMOTE_URL"
P=$(git -C "$APP" push origin main 2>&1; echo "rc=$?")
ok "the hook refuses it" "$P" "Do not work around this"
ok "  non-zero" "$P" "rc=1"
P2=$(git -C "$APP" push --no-verify origin main 2>&1; echo "rc=$?")
ok "--no-verify gets past the hook — exactly the hole the card discloses" "$P2" "rc=0"
OUT=$(probe lock_state "$APP")
ok "the state reports the explicit pushurl as drift" "$(printf '%s' "$OUT" | jqf lock.drift)" "explicit-pushurl:origin"
D=$(probe doctor_locks "$APP" | tr -d '\n')
ok "Doctor offers to neutralise it" "$D" "lock-neutralise-pushurl:"
OUT=$(probe lock_fix "$APP" "lock-neutralise-pushurl:$APP")
ok "neutralising stores and removes the pushurl" "$(printf '%s' "$OUT" | jqf message)" "stored and removed"
ok "  the value is kept for unlock" "$(git -C "$APP" config --get gitswitch.savedpushurl.origin)" "$REMOTE_URL"
echo y >> "$APP/a.txt"; git -C "$APP" commit -qam "y"
P3=$(git -C "$APP" push --no-verify origin main 2>&1; echo "rc=$?")
ok "now even --no-verify is refused: the rewrite applies again" "$P3" "GitSwitch: push refused."

section "Tampering with the user-level mirrors is healed"
git -C "$APP" config --local --unset gitswitch.pushBlocked
git -C "$APP" config --local --remove-section 'url.gitswitch-push-blocked://'
rm "$APP/.git/hooks/pre-push"
OUT=$(probe lock_state "$APP")
ok "the next look heals them" "$(printf '%s' "$OUT" | jqf lock.mirrors)" "healed"
ok "  the flag is back" "$(git -C "$APP" config --local --get gitswitch.pushBlocked)" "true"
ok "  the hook is back" "$([ -x "$APP/.git/hooks/pre-push" ] && echo yes || echo no)" "yes"
ok "  the event names what drifted" "$(printf '%s' "$OUT" | jqf lock.recent_events)" "mirror-flag"
OUT=$(probe lock_state "$APP")
ok "a second look is plain ok" "$(printf '%s' "$OUT" | jqf lock.mirrors)" "ok"

section "The guard-rail cannot undo a lock"
OUT=$(probe guardrail_off "$APP")
ok "turning the guard-rail off is refused while locked" "$OUT" "Unlock it first"
OUT=$(probe push_mode "$APP" "guardrail")
ok "so is switching to guard-rail mode" "$OUT" "Unlock it first"
ok "  and the repo is still locked" "$(git -C "$APP" config --get gitswitch.locked)" "true"

section "A cancelled, denied or failed prompt changes nothing"
OUT=$(GITSWITCH_ELEVATE=cancelled probe push_mode "$APP" "allowed")
ok "cancelled is reported as cancelled" "$(printf '%s' "$OUT" | jqf outcome)" "cancelled"
ok "  in a sentence" "$(printf '%s' "$OUT" | jqf message)" "Nothing changed."
ok "  and the repo stays locked" "$(printf '%s' "$OUT" | jqf state.lock.locked)" "True"
OUT=$(GITSWITCH_ELEVATE=denied probe push_mode "$APP" "allowed")
ok "a wrong password is reported as denied" "$(printf '%s' "$OUT" | jqf outcome)" "denied"
OUT=$(GITSWITCH_ELEVATE=failed probe push_mode "$APP" "allowed")
ok "a failure is reported as failed" "$(printf '%s' "$OUT" | jqf outcome)" "failed"
OUT=$(GITSWITCH_ELEVATE=manual probe push_mode "$APP" "allowed")
ok "with no prompt available, the exact command is shown" "$(printf '%s' "$OUT" | jqf command)" "sudo '"
ok "  bound to the job's checksum" "$(printf '%s' "$OUT" | jqf command)" "--sha256 "
ok "  still locked" "$(git -C "$APP" config --get gitswitch.locked)" "true"

section "Doctor sees a missing helper; its fix reinstalls it"
sys_rm "$HELPER"
D=$(probe doctor_locks "$APP" | tr -d '\n')
ok "Doctor reports the missing helper" "$D" "push-lock-helper-missing"
OUT=$(probe lock_state "$APP")
ok "the state needs an administrator" "$(printf '%s' "$OUT" | jqf lock.needs_elevation)" "True"
ok "  and says why" "$(printf '%s' "$OUT" | jqf lock.drift)" "helper-missing"
P=$(git -C "$APP" push origin main 2>&1; echo "rc=$?")
if [ "$PLAT" = windows ]; then
  # The remote helper is a full copy on Windows, so the explanation survives.
  ok "  the lock itself still holds" "$P" "GitSwitch: push refused."
  ok "  the state still sees the explanation program" "$(printf '%s' "$OUT" | jqf lock.remote_helper)" "ok"
else
  # A dangling symlink: the refusal comes from the system-scope rewrite and
  # needs no program at all; only the wording is git's.
  ok "  the lock itself still holds" "$P" "remote helper 'gitswitch-push-blocked' aborted session"
  ok "  (with git's wording, since the explaining program is gone)" "$P" "rc=128"
  ok "  the state reports the explanation as missing too" "$(printf '%s' "$OUT" | jqf lock.remote_helper)" "missing"
fi
OUT=$(probe lock_fix "$APP" "lock-bootstrap")
ok "the fix bootstraps again" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
ok "  the helper is back" "$([ -x "$HELPER" ] && echo yes || echo no)" "yes"
D=$(probe doctor_locks "$APP" | tr -d '\n')
ok "Doctor is quiet about the helper again" "$(printf '%s' "$D" | grep -c 'push-lock-helper-missing' || true)" "0"

section "Doctor: a locked repository that was deleted"
git clone -q "$SB/remote.git" "$SB/gone" 2>/dev/null
probe push_mode "$SB/gone" "locked" >/dev/null
rm -rf "$SB/gone"
D=$(probe doctor_locks "$APP" | tr -d '\n')
ok "Doctor reports the vanished repository" "$D" "push-lock-repo-missing:"
OUT=$(probe lock_fix "$APP" "lock-forget:$SB/gone")
ok "forgetting it is one administrator job" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
D=$(probe doctor_locks "$APP" | tr -d '\n')
ok "  and Doctor is quiet" "$(printf '%s' "$D" | grep -c 'push-lock-repo-missing' || true)" "0"

section "Unlocking from the app restores everything"
OUT=$(probe push_mode "$APP" "allowed")
ok "the outcome is applied" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
ok "  not locked" "$(printf '%s' "$OUT" | jqf state.lock.locked)" "False"
ok "  pushes allowed" "$(printf '%s' "$OUT" | jqf state.blocked)" "False"
ok "  the flag is gone" "$(git -C "$APP" config --local --get gitswitch.pushBlocked || echo unset)" "unset"
ok "  the hook is gone" "$([ -e "$APP/.git/hooks/pre-push" ] && echo present || echo gone)" "gone"
ok "  the stored pushurl was put back" "$(git -C "$APP" config --get remote.origin.pushurl)" "$REMOTE_URL"
P=$(git -C "$APP" push origin main 2>&1; echo "rc=$?")
ok "a terminal push works again" "$P" "rc=0"

if [ -z "$REAL" ]; then
section "A symlinked /etc, as on macOS, is followed when the link is the administrator's"
# The registry and system gitconfig live under $ROOT/etc; make that a symlink
# to $ROOT/private/etc the way macOS lays out /etc -> private/etc. (In real
# mode on macOS this happens naturally on every lock.)
mv "$LOCK_ROOT/etc" "$LOCK_ROOT/private_etc_real"; mkdir -p "$LOCK_ROOT/private"; mv "$LOCK_ROOT/private_etc_real" "$LOCK_ROOT/private/etc"
ln -s "private/etc" "$LOCK_ROOT/etc"
OUT=$(probe push_mode "$APP" "locked")
ok "locking through the symlinked /etc works" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
ok "  the marker landed in the real file" "$(grep -c 'GitSwitch push lock' "$LOCK_ROOT/private/etc/gitconfig")" "2"
ok "  and git sees the rule" "$(git -C "$APP" config --show-scope --get gitswitch.locked)" "system	true"
OUT=$(probe push_mode "$APP" "allowed")
ok "unlocking through it works too" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
fi

section "An outdated installed helper is replaced before the job runs"
# Stand in a different, working executable as the "installed" helper and make
# the registry vouch for it: the bundled copy is then newer.
sys_cp "$(command -v bash)" "$HELPER"
$SUDO "$PY" - "$REG/locks.json" "$(sha "$HELPER")" <<'PYE'
import json,sys
p=sys.argv[1]; d=json.load(open(p)); d["helper"]["sha256"]=sys.argv[2]; json.dump(d,open(p,"w"),indent=2)
PYE
OUT=$(probe lock_state "$APP")
ok "the state reports the helper as outdated" "$(printf '%s' "$OUT" | jqf lock.helper)" "outdated"
OUT=$(probe push_mode "$APP" "locked")
ok "locking re-installs the bundled helper first and applies" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
ok "  the installed copy is the bundled one again" "$(cmp -s "$HELPER" "$HELPER_BUNDLED" && echo same || echo differs)" "same"
ok "  and the state says ok" "$(printf '%s' "$OUT" | jqf state.lock.helper)" "ok"
ok "  the audit log recorded the reinstall" "$(grep -c 'bootstrap installed helper' "$AUDIT")" "3"
OUT=$(probe push_mode "$APP" "allowed")
ok "unlock after the upgrade" "$(printf '%s' "$OUT" | jqf outcome)" "applied"

section "Helper status and uninstall from the app"
OUT=$(probe lock_helper_status "$APP")
ok "the helper is reported installed" "$(printf '%s' "$OUT" | jqf installed)" "True"
ok "  and vouched for" "$(printf '%s' "$OUT" | jqf helper)" "ok"
ok "  the registry is ok" "$(printf '%s' "$OUT" | jqf registry)" "ok"
OUT=$(probe lock_uninstall "$APP")
ok "uninstall from the app applies" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
ok "  the helper is gone" "$([ -e "$HELPER" ] && echo present || echo gone)" "gone"
ok "  the registry is gone" "$(reg_left)" "gone"

section "A block on a superproject reaches its submodules — the repositories the code lives in"
SUP="$SB/super-cover"; for r in remote lib lib2 inner; do git init -q --bare "$SB/cover-$r.git"; done
for r in lib lib2 inner; do git clone -q "$SB/cover-$r.git" "$SB/cover-author-$r" 2>/dev/null; printf '%s v1\n' "$r" > "$SB/cover-author-$r/$r.txt"; git -C "$SB/cover-author-$r" add -A >/dev/null; git -C "$SB/cover-author-$r" commit -qm "$r: initial"; git -C "$SB/cover-author-$r" push -q origin main 2>/dev/null; done
# lib has a submodule of its own.
( cd "$SB/cover-author-lib" && git submodule add -q "$SB/cover-inner.git" inner 2>/dev/null ); git -C "$SB/cover-author-lib" commit -qam "lib: add inner" >/dev/null; git -C "$SB/cover-author-lib" push -q origin main 2>/dev/null
git clone -q "$SB/cover-remote.git" "$SB/cover-author" 2>/dev/null; printf 'readme\n' > "$SB/cover-author/README.md"; git -C "$SB/cover-author" add -A >/dev/null; git -C "$SB/cover-author" commit -qm "super: initial"
( cd "$SB/cover-author" && git submodule add -q "$SB/cover-lib.git" lib 2>/dev/null ); git -C "$SB/cover-author" commit -qam "super: add lib" >/dev/null; git -C "$SB/cover-author" push -q origin main 2>/dev/null
git clone -q --recurse-submodules "$SB/cover-remote.git" "$SUP" 2>/dev/null; git -C "$SUP/lib" checkout -q main; git -C "$SUP/lib/inner" checkout -q main
printf 'lib v2\n' > "$SUP/lib/lib.txt"; git -C "$SUP/lib" commit -qam "lib: v2"
cov() { pinvoke changes_push_coverage "{\"repoPath\":\"$SUP\"}"; }
OUT=$(probe push_mode "$SUP" "guardrail")
ok "blocking the superproject applies" "$(printf '%s' "$OUT" | jqf outcome):$(printf '%s' "$OUT" | jqf state.repo_blocked)" "applied:True"
ok "  and reaches every populated submodule, nested ones included" "$(cov | "$PY" -c 'import sys,json;c=json.load(sys.stdin)["value"];print(c["mode"], [(s["path"],s["blocked"],s["covered"]) for s in c["submodules"]], c["uncovered"], c["summary"])')" "guardrail [('lib', True, True), ('lib/inner', True, True)] [] Covers this repository and 2 submodules."
ok "  the submodule carries the flag and the hook" "$(git -C "$SUP/lib" config --local --get gitswitch.pushBlocked):$([ "$(grep -c 'GitSwitch push guard' "$(git -C "$SUP/lib" rev-parse --git-path hooks)/pre-push")" -gt 0 ] && echo hook || echo nohook)" "true:hook"
ok "  marked as the superproject's doing" "$(git -C "$SUP/lib" config --local --get gitswitch.pushBlockedBy)" "superproject"
git -C "$SUP/lib" push origin main >/dev/null 2>"$SB/cover-push.err"; RC=$?
ok "a push from inside the submodule is refused in a plain terminal" "$([ "$RC" -ne 0 ] && echo refused || echo PUSHED):$(git -C "$SB/cover-lib.git" log --format=%s -1 main)" "refused:lib: add inner"
ok "  by GitSwitch, not by chance" "$([ "$(grep -ci gitswitch "$SB/cover-push.err")" -gt 0 ] && echo yes || echo no)" "yes"
git -C "$SUP/lib" config --local --unset gitswitch.pushBlocked; rm -f "$(git -C "$SUP/lib" rev-parse --git-path hooks)/pre-push"
ok "a submodule whose pieces were removed is reported as not covered" "$(cov | "$PY" -c 'import sys,json;c=json.load(sys.stdin)["value"];print(c["needs_repair"], c["uncovered"], c["summary"])')" "True ['lib'] Not covering 1 of 2 submodules: lib (only its address rewrite still stops a push; Repair re-applies the rest)."
OUT=$(pinvoke changes_repair_push_block "{\"repoPath\":\"$SUP\"}")
ok "  Repair covers it again" "$(cov | "$PY" -c 'import sys,json;c=json.load(sys.stdin)["value"];print(c["needs_repair"], c["uncovered"])')" "False []"
# A submodule added upstream and downloaded under the block gets it at once.
( cd "$SB/cover-author" && git submodule add -q "$SB/cover-lib2.git" lib2 2>/dev/null ); git -C "$SB/cover-author" commit -qam "super: add lib2" >/dev/null; git -C "$SB/cover-author" push -q origin main 2>/dev/null
git -C "$SUP" pull -q --ff-only origin main 2>/dev/null
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$SUP\",\"paths\":[\"lib2\"]}")
ok "a submodule downloaded under the block gets it at once, and the result says so" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], "got the same guard-rail" in v["detail"])'):$(git -C "$SUP/lib2" config --local --get gitswitch.pushBlocked)" "True True:true"
ok "  and counts as covered" "$(cov | jqf value.summary)" "Covers this repository and 3 submodules."
# One the person blocks themselves AFTER the superproject's block reached it
# keeps its block when the superproject allows pushes again: acting on a
# repository makes its block theirs.
ok "(lib2 carries the superproject's mark before the person acts on it)" "$(git -C "$SUP/lib2" config --local --get gitswitch.pushBlockedBy)" "superproject"
OUT=$(probe push_mode "$SUP/lib2" "guardrail")
ok "blocking a submodule yourself makes its block yours: the superproject's mark is gone" "$(git -C "$SUP/lib2" config --local --get gitswitch.pushBlockedBy || echo unmarked)" "unmarked"
OUT=$(probe push_mode "$SUP" "allowed")
ok "allowing the superproject takes the block away from the submodules it covered" "$(printf '%s' "$OUT" | jqf outcome):$(git -C "$SUP/lib" config --local --get gitswitch.pushBlocked || echo unset):$(git -C "$SUP/lib/inner" config --local --get gitswitch.pushBlocked || echo unset):$([ -e "$(git -C "$SUP/lib" rev-parse --git-path hooks)/pre-push" ] && echo hook || echo nohook)" "applied:unset:unset:nohook"
ok "  but not from one blocked on its own" "$(git -C "$SUP/lib2" config --local --get gitswitch.pushBlocked)" "true"
ok "  and the card says which ones are" "$(cov | jqf value.summary)" "Blocked on their own: lib2."
git -C "$SUP/lib" push -q origin main 2>/dev/null
ok "  a push from inside lib goes through again" "$(git -C "$SB/cover-lib.git" log --format=%s -1 main)" "lib: v2"
# A submodule the person blocked BEFORE the superproject's block keeps it afterwards: the superproject never marks what was already theirs.
OUT=$(probe push_mode "$SUP/lib" "guardrail")
OUT=$(probe push_mode "$SUP" "guardrail"); OUT=$(probe push_mode "$SUP" "allowed")
ok "a submodule blocked on its own before the superproject's block keeps it when the superproject allows again" "$(git -C "$SUP/lib" config --local --get gitswitch.pushBlocked):$(git -C "$SUP/lib" config --local --get gitswitch.pushBlockedBy || echo unmarked)" "true:unmarked"
OUT=$(probe push_mode "$SUP/lib" "allowed")
# A moved superproject still takes its block away from the submodules it covered: the marker names no path.
OUT=$(probe push_mode "$SUP" "guardrail"); mv "$SUP" "$SUP-moved"; OUT=$(probe push_mode "$SUP-moved" "allowed")
ok "allowing a superproject that was moved still uncovers its submodules" "$(printf '%s' "$OUT" | jqf outcome):$(git -C "$SUP-moved/lib" config --local --get gitswitch.pushBlocked || echo unset)" "applied:unset"
mv "$SUP-moved" "$SUP"

section "A lock on a superproject locks its submodules in the same job; unlocking frees them all"
git -C "$SUP/lib2" config --local --unset gitswitch.pushBlocked; rm -f "$(git -C "$SUP/lib2" rev-parse --git-path hooks)/pre-push"
OUT=$(probe push_mode "$SUP" "locked")
ok "locking the superproject applies" "$(printf '%s' "$OUT" | jqf outcome):$(printf '%s' "$OUT" | jqf state.lock.locked)" "applied:True"
for sub in lib lib/inner lib2; do ST=$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/$sub\"}"); ok "  $sub is locked too, with its mirrors in place" "$(printf '%s' "$ST" | jqf value.lock.locked):$(printf '%s' "$ST" | jqf value.lock.mirrors)" "True:ok"; done
ok "  the coverage says so" "$(cov | jqf value.summary)" "Covers this repository and 3 submodules."
evs() { for sub in lib lib/inner lib2; do pinvoke changes_push_state "{\"repoPath\":\"$SUP/$sub\"}" | "$PY" -c 'import sys,json;e=json.load(sys.stdin)["value"]["lock"]["recent_events"];print(" ".join(sorted(set(x["code"] for x in e))), "|", e[0]["detail"] if e else "")'; done | tr -d '\r'; }
ok "  each submodule's own event log records its lock, and says who did it" "$(evs | tr '\n' ';')" "locked | locked with its superproject super-cover;locked | locked with its superproject super-cover;locked | locked with its superproject super-cover;"
ok "  the registry lists the four of them" "$(pinvoke push_lock_helper_status "{}" | "$PY" -c 'import sys,json;print(len([l for l in json.load(sys.stdin)["value"]["locks"] if "super-cover" in l["repo"]]))')" "4"
# A submodule the lock does not reach (unlocked from inside it, by the person):
# Repair adds it, and its own log says Repair did it, for which superproject.
OUT=$(probe push_mode "$SUP/lib2" "allowed")
ok "(lib2 unlocked from inside it)" "$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib2\"}" | jqf value.lock.locked)" "False"
OUT=$(pinvoke changes_repair_push_lock "{\"repoPath\":\"$SUP\"}")
ok "Repair locks it again, and its own log says Repair did it, for which superproject" "$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib2\"}" | "$PY" -c 'import sys,json;l=json.load(sys.stdin)["value"]["lock"];print(l["locked"], l["recent_events"][0]["detail"])')" "True locked by Repair with its superproject super-cover"
# Read while the submodules' pieces are re-applied (a lock job's last step, a
# Repair), in one process as in the app: each repository's measurements and
# writes take turns, so this finishes — and calls nothing sound broken.
OUT=$(probe coverage_while_covering "$SUP" 12)
ok "coverage read while the submodules' pieces are re-applied finishes, and calls no sound lock broken" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin);print(len(v["flagged"]), len(v["notes"]), v["flagged"][:1])')" "0 0 []"
inode() { case "$PLAT" in macos) stat -f %i "$1";; *) stat -c %i "$1";; esac; }
CFG="$(git -C "$SUP/lib" rev-parse --path-format=absolute --git-path config)"
BEFORE=$(inode "$CFG")
OUT=$(pinvoke changes_repair_push_lock "{\"repoPath\":\"$SUP\"}")
ok "  re-applying a complete block leaves a submodule's config file as it was" "$(printf '%s' "$OUT" | jqf value.outcome):$([ "$(inode "$CFG")" = "$BEFORE" ] && echo untouched || echo rewritten)" "applied:untouched"
KEY='url.gitswitch-push-blocked://.pushInsteadOf'
WANT=$(git -C "$SUP/lib" config --local --get-all "$KEY" | sort | tr '\n' ' ')
git -C "$SUP/lib" config --local --add "$KEY" "git@github.com:"; git -C "$SUP/lib" config --local --add "$KEY" "ssh://stale.example/"
OUT=$(pinvoke changes_repair_push_lock "{\"repoPath\":\"$SUP\"}")
ok "  a copy and a prefix no remote needs, left by an older apply, are cleaned up" "$(git -C "$SUP/lib" config --local --get-all "$KEY" | sort | tr '\n' ' ')" "$WANT"
OUT=$(probe push_mode "$SUP" "allowed")
ok "unlocking the superproject unlocks them all" "$(printf '%s' "$OUT" | jqf outcome):$(for sub in lib lib/inner lib2; do pinvoke changes_push_state "{\"repoPath\":\"$SUP/$sub\"}" | jqf value.lock.locked; done | tr -d '\r' | tr '\n' ',')" "applied:False,False,False,"
ok "  and nothing of the block is left in them" "$(git -C "$SUP/lib" config --local --get gitswitch.pushBlocked || echo unset):$(git -C "$SUP/lib2" config --local --get gitswitch.pushBlocked || echo unset)" "unset:unset"
ok "  each submodule's own event log records the unlock too" "$(evs | tr '\n' ';')" "locked unlocked | unlocked with its superproject super-cover;locked unlocked | unlocked with its superproject super-cover;locked unlocked | unlocked with its superproject super-cover;"

if [ "$PLAT" != windows ]; then
section "A lock finished by hand (no administrator prompt) does what the prompt does"
# lib2 is blocked by the person before the superproject is locked by hand.
OUT=$(probe push_mode "$SUP/lib2" "guardrail")
OUT=$(GITSWITCH_ELEVATE=manual probe push_mode "$SUP" "locked")
ok "with no prompt available the lock is handed over as a command" "$(printf '%s' "$OUT" | jqf outcome)" "manual-required"
CMD="$(printf '%s' "$OUT" | jqf command)"; NONCE="$(printf '%s' "$OUT" | jqf job_nonce)"
# The person runs it as an administrator, then says so: through `sudo -n` on a
# REAL runner, unprivileged under the test root (where $SUDO is empty).
eval "$SUDO ${CMD#sudo }" >/dev/null 2>&1
OUT=$(pinvoke push_lock_finish_manual "{\"nonce\":\"$NONCE\"}")
ok "  and \"I ran it\" finishes it" "$(printf '%s' "$OUT" | jqf value.outcome):$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib\"}" | jqf value.lock.locked)" "applied:True"
ok "  a submodule the lock reached is marked as the superproject's" "$(git -C "$SUP/lib" config --local --get gitswitch.pushBlockedBy)" "superproject"
ok "  one the person had blocked on their own stays theirs" "$(git -C "$SUP/lib2" config --local --get gitswitch.pushBlockedBy || echo unmarked)" "unmarked"
ok "  and each submodule's own log names the superproject" "$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib\"}" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["lock"]["recent_events"][0]["detail"])')" "locked (finished by hand) with its superproject super-cover"
OUT=$(GITSWITCH_ELEVATE=manual probe push_mode "$SUP" "allowed")
CMD="$(printf '%s' "$OUT" | jqf command)"; NONCE="$(printf '%s' "$OUT" | jqf job_nonce)"
eval "$SUDO ${CMD#sudo }" >/dev/null 2>&1
OUT=$(pinvoke push_lock_finish_manual "{\"nonce\":\"$NONCE\"}")
ok "an unlock finished by hand frees what the superproject's lock reached" "$(printf '%s' "$OUT" | jqf value.outcome):$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib\"}" | jqf value.lock.locked):$(git -C "$SUP/lib" config --local --get gitswitch.pushBlocked || echo unset)" "applied:False:unset"
ok "  but not the one the person locked on their own" "$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib2\"}" | jqf value.lock.locked)" "True"
OUT=$(probe push_mode "$SUP/lib2" "allowed")
fi

section "A submodule downloaded under a lock gets the lock itself, in one job"
OUT=$(probe push_mode "$SUP" "locked")
ok "(the superproject is locked)" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
# A submodule whose own settings cannot be written back: Repair says so and
# names it — never "Everything is in place." — and does it once it can.
GD="$(git -C "$SUP/lib2" rev-parse --absolute-git-dir)"
git -C "$SUP/lib2" config --local --unset gitswitch.pushBlocked
chmod 555 "$GD"
# Windows ignores a folder's read-only bit (and the CI runner is the administrator):
# there the folder stays writable, so this case is checked on macOS and Linux.
if ( : > "$GD/.gitswitch-ro-probe" ) 2>/dev/null; then
  rm -f "$GD/.gitswitch-ro-probe"
  RO_SKIP=1
else
  RO_SKIP=
fi
OUT=$(pinvoke changes_repair_push_lock "{\"repoPath\":\"$SUP\"}")
COV=$(cov)
chmod 755 "$GD"
# A disclosed unprotected remote-helper directory (EXPECT_DRIFT, above: the app's
# own rule) is drift every repository shows, this submodule too.
DRIFT_WANT="[['mirror-flag']]"
case "$EXPECT_DRIFT" in *remote-helper-unprotected-dir*) DRIFT_WANT="[['remote-helper-unprotected-dir', 'mirror-flag']]";; esac
if [ -n "$RO_SKIP" ]; then
  echo "  (skipped on $PLAT: a folder can't be made read-only here, so a Repair that can't write back is checked on macOS and Linux)"
else
ok "a submodule Repair cannot write back is named as not covered, and why — not called in place" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];m=v["message"];print(v["outcome"], "lib2 (" in m, "Everything is in place" in m, "Could not write: lib2" in m)')" "incomplete True False True"
ok "  the card's Repair for it promises no administrator prompt (none is needed)" "$(printf '%s' "$COV" | "$PY" -c 'import sys,json;c=json.load(sys.stdin)["value"];print(c["needs_repair"], c["needs_elevation"], [s["drift"] for s in c["submodules"] if s["path"]=="lib2"])')" "True False $DRIFT_WANT"
fi
OUT=$(pinvoke changes_repair_push_lock "{\"repoPath\":\"$SUP\"}")
ok "  and once it can be written, Repair puts it back" "$(printf '%s' "$OUT" | jqf value.outcome):$(git -C "$SUP/lib2" config --local --get gitswitch.pushBlocked)" "applied:true"
git init -q --bare "$SB/cover-lib3.git"; git clone -q "$SB/cover-lib3.git" "$SB/cover-author-lib3" 2>/dev/null
printf 'lib3 v1\n' > "$SB/cover-author-lib3/lib3.txt"; git -C "$SB/cover-author-lib3" add -A >/dev/null; git -C "$SB/cover-author-lib3" commit -qm "lib3: initial"; git -C "$SB/cover-author-lib3" push -q origin main 2>/dev/null
( cd "$SB/cover-author" && git submodule add -q "$SB/cover-lib3.git" lib3 2>/dev/null ); git -C "$SB/cover-author" commit -qam "super: add lib3" >/dev/null; git -C "$SB/cover-author" push -q origin main 2>/dev/null
git -C "$SUP" pull -q --ff-only origin main 2>/dev/null
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$SUP\",\"paths\":[\"lib3\"]}")
ST=$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib3\"}")
ok "a submodule downloaded under the lock is locked at once, and the result says so" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], "were locked too" in v["detail"], "needs Repair" in v["detail"])'):$(printf '%s' "$ST" | jqf value.lock.locked)" "True True False:True"
ok "  marked as the superproject's doing" "$(git -C "$SUP/lib3" config --local --get gitswitch.pushBlockedBy)" "superproject"
ok "  and its own log says so" "$(printf '%s' "$ST" | "$PY" -c 'import sys,json;print(json.load(sys.stdin)["value"]["lock"]["recent_events"][0]["detail"])')" "locked when downloaded"
# Its module directory keeps the lock: downloaded again after a deinit, it is still locked — and said to be.
git -C "$SUP" submodule deinit -q -f lib3 2>/dev/null
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$SUP\",\"paths\":[\"lib3\"]}")
ok "a submodule downloaded again into a module directory that kept the lock is said to be locked" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["ok"], "were locked too" in v["detail"], "not the lock" in v["detail"])'):$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib3\"}" | jqf value.lock.locked)" "True True False:True"
OUT=$(probe push_mode "$SUP" "allowed")
ok "unlocking the superproject unlocks it too" "$(pinvoke changes_push_state "{\"repoPath\":\"$SUP/lib3\"}" | jqf value.lock.locked)" "False"
# An interrupted download under a blocked superproject: Download clears the
# leftovers and fetches it again — and the fresh copy gets the guard-rail.
OUT=$(probe push_mode "$SUP" "guardrail")
ok "(the superproject's pushes are blocked)" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
git init -q --bare "$SB/cover-lib4.git"; git clone -q "$SB/cover-lib4.git" "$SB/cover-author-lib4" 2>/dev/null
printf 'lib4 v1\n' > "$SB/cover-author-lib4/lib4.txt"; git -C "$SB/cover-author-lib4" add -A >/dev/null; git -C "$SB/cover-author-lib4" commit -qm "lib4: initial"; git -C "$SB/cover-author-lib4" push -q origin main 2>/dev/null
( cd "$SB/cover-author" && git submodule add -q "$SB/cover-lib4.git" lib4 2>/dev/null ); git -C "$SB/cover-author" commit -qam "super: add lib4" >/dev/null; git -C "$SB/cover-author" push -q origin main 2>/dev/null
git -C "$SUP" pull -q --ff-only origin main 2>/dev/null
# What a crash mid-download leaves: the gitfile and a module directory with no commit.
mkdir -p "$SUP/.git/modules"; git init -q --separate-git-dir "$SUP/.git/modules/lib4" "$SUP/lib4" 2>/dev/null
OUT=$(pinvoke changes_submodule_update "{\"repoPath\":\"$SUP\",\"paths\":[\"lib4\"]}")
ok "an interrupted download finished under a block gets the block too" "$(printf '%s' "$OUT" | jqf value.ok):$(git -C "$SUP/lib4" config --local --get gitswitch.pushBlockedBy)" "True:superproject"
OUT=$(probe push_mode "$SUP" "allowed")

section "The run ends with the helper uninstalled, as it began"
OUT=$(probe lock_uninstall "$APP")
ok "uninstall applies" "$(printf '%s' "$OUT" | jqf outcome)" "applied"
ok "  the helper is gone" "$([ -e "$HELPER" ] && echo present || echo gone)" "gone"
ok "  the registry is gone" "$(reg_left)" "gone"

# Real mode: leave the machine as it was found.
if [ -n "$REAL" ]; then
  if [ -n "$SYSCFG_ORIG" ]; then sys_write "$SYSCFG" < "$SYSCFG_ORIG"; else sys_rm "$SYSCFG"; fi
  ok "REAL: the system gitconfig is back to what the machine had" "$([ -n "$SYSCFG_ORIG" ] && (cmp -s "$SYSCFG" "$SYSCFG_ORIG" && echo restored) || ([ ! -e "$SYSCFG" ] && echo restored))" "restored"
fi

verify_result
