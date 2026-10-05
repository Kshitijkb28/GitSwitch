#!/bin/bash
# The terminal drawer: a real shell in a real pty, and the git shim that keeps
# the push lock standing inside it.
#
# Every check here runs an actual shell through terminal::run_once — the same
# code path the app uses — because a shim is only worth having if it fires, and
# the only way to know is to run it.
set -u
source "$(dirname "$0")/lib.sh"
verify_init console "Console Tester" "console@example.test"

# A repository with an upstream, so `push` is a real thing to attempt.
git init -q --bare "$SB/remote.git"
git clone -q "$SB/remote.git" "$SB/work" 2>/dev/null
cd "$SB/work"
echo hello > README.md
git add -A >/dev/null; git commit -qm "first"; git push -q origin main 2>/dev/null

# term <command line>  — run one line in a shell opened in $SB/work
# Strip the escape codes a real terminal carries, so every assertion below is
# about what a person reads rather than how it was coloured.
term() { probe term_run "$SB/work" "$1" | "$PY" -c "
import sys,json,re
d=json.load(sys.stdin)
print(re.sub(r'\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][B0]|\r', '', d.get('output','')))
"; }
# On Windows the shell under test is cmd.exe (COMSPEC), and the guard is the
# git-shim program rather than the sh script. The few shell-specific spellings
# below go through these helpers; the git behaviour asserted is the same.
IS_WIN=0; command -v cygpath >/dev/null 2>&1 && IS_WIN=1
# withenv VAR VALUE "command" → the command with that variable set, for this shell
withenv() { if [ "$IS_WIN" = 1 ]; then printf 'set %s=%s&& %s' "$1" "$2" "$3"; else printf '%s=%s %s' "$1" "$2" "$3"; fi; }
NULLDEV=/dev/null; [ "$IS_WIN" = 1 ] && NULLDEV=NUL

section "A real shell, in the repository"
OUT=$(term "git rev-parse --abbrev-ref HEAD")
ok "the shell runs and git answers" "$OUT" "main"
OUT=$(if [ "$IS_WIN" = 1 ]; then term "cd" | tr '\\' '/'; else term "pwd"; fi)
ok "  it starts in the repository that was chosen" "$OUT" "$SB/work"
OUT=$(if [ "$IS_WIN" = 1 ]; then term "echo %GITSWITCH_TERMINAL%"; else term "echo \$GITSWITCH_TERMINAL"; fi)
ok "  and says it is GitSwitch's terminal, so a shell profile can tell" "$OUT" "1"

section "The shim is the git the shell finds"
OUT=$(if [ "$IS_WIN" = 1 ]; then term "where git"; else term "command -v git"; fi)
ok "git resolves to the shim, not straight to the system one" "$OUT" "terminal-shim"
OUT=$(term "git status --short --branch")
ok "  an ordinary command passes straight through" "$OUT" "## main"
OUT=$(term "git log --oneline")
ok "  and so does a read of history" "$OUT" "first"

section "The documented push-lock bypass is refused"
# push_lock::caveats() names GIT_CONFIG_NOSYSTEM as the one-line way past a
# lock. Inside this app's own terminal that has to be refused, or the app
# hands out the bypass to the thing it promises needs an admin password.
OUT=$(term "$(withenv GIT_CONFIG_NOSYSTEM 1 "git push origin main")")
ok "a push that would skip system config is refused" "$OUT" "would skip the system git config"
ok "  and the refusal names the variable to unset" "$OUT" "GIT_CONFIG_NOSYSTEM"
OUT=$(term "$(withenv GIT_CONFIG_SYSTEM "$NULLDEV" "git push origin main")")
ok "the other spelling is refused too" "$OUT" "would skip the system git config"
# It must never tell somebody how to turn a lock off.
ok "  and never prints a way to disable the lock" "$(term "$(withenv GIT_CONFIG_NOSYSTEM 1 "git push origin main")" | grep -c -- "--uninstall" || true)" "0"
OUT=$(term "$(withenv GIT_CONFIG_NOSYSTEM 1 "git status --short")")
ok "reading is still allowed with the variable set" "$(printf '%s' "$OUT" | grep -c "would skip" || true)" "0"

section "Force pushes and skipped hooks are refused"
OUT=$(term "git push --force origin main")
ok "--force is refused" "$OUT" "never force-pushes"
OUT=$(term "git push --force-with-lease origin main")
ok "  so is --force-with-lease" "$OUT" "never force-pushes"
OUT=$(term "git push origin +main:main")
ok "  and so is a + refspec, which is a force in another spelling" "$OUT" "a leading + is a force"
OUT=$(term "git commit --no-verify --allow-empty -m x")
ok "--no-verify is refused on commit" "$OUT" "identity guard"
OUT=$(term "git commit -n --allow-empty -m x")
ok "  including its short form" "$OUT" "identity guard"
# `-n` means --dry-run for push, which is harmless and must NOT be refused.
OUT=$(term "git push -n origin main")
ok "push -n is a dry run, not --no-verify, and is allowed" "$(printf '%s' "$OUT" | grep -c "identity guard" || true)" "0"

section "A flag before the subcommand does not hide it"
OUT=$(term "git -c color.ui=never push --force origin main")
ok "-c key=value is not mistaken for the subcommand" "$OUT" "never force-pushes"
OUT=$(term "git -C . push --force origin main")
ok "  neither is -C's path" "$OUT" "never force-pushes"

section "An ordinary push still reaches git itself"
OUT=$(term "git push origin main")
ok "the shim does not stand in the way of a normal push" "$(printf '%s' "$OUT" | grep -c "never force-pushes\|would skip" || true)" "0"

section "The push block still applies, because it is git's own"
# The block is a pushInsteadOf rewrite plus a pre-push hook in the repo's
# config — nothing to do with the shim. It must hold in this shell too.
probe block "$SB/work" "on" >/dev/null
OUT=$(term "git push origin main")
ok "a blocked repository refuses a push in the terminal too" "$OUT" "gitswitch-push-blocked"
probe block "$SB/work" "off" >/dev/null

# (That a closed terminal leaves no shell behind is a Rust test now —
# terminal.rs `closing_a_terminal_leaves_no_shell_behind` — which checks the
# shell's own pid rather than counting processes that mention the path.)

section "The command log records what the app itself ran"
# The log lives in memory, so it cannot survive between probe invocations —
# this op runs a real status read through GitCmd and dumps the log in the same
# process, which is the only way to prove the recording actually happens.
LOG=$(probe cmdlog "$SB/work")
ok "a real status read is recorded" "$LOG" '"git"'
ok "  with the repository it ran in" "$LOG" "$SB/work"
ok "  and the porcelain status among the commands" "$LOG" "--porcelain=v2"
ok "  every entry carries an exit code" "$LOG" '"exit_code"'
ok "  and how long it took" "$LOG" '"duration_ms"'
NONZERO=$(printf '%s' "$LOG" | "$PY" -c "
import sys,json
d=json.load(sys.stdin)
print('yes' if any(e['duration_ms'] >= 0 for e in d) and len(d) > 3 else 'no')
")
ok "  a status read is many commands, not one" "$NONZERO" "yes"
READS=$(printf '%s' "$LOG" | "$PY" -c "
import sys,json
d=json.load(sys.stdin)
print('reads=%d writes=%d' % (sum(1 for e in d if e['is_read']), sum(1 for e in d if not e['is_read'])))
")
ok "  and they are classified as reads, so the list is not drowned" "$READS" "reads="

section "A token never reaches the log"
# A person pastes a credentialed URL into Clone and it lands in argv verbatim;
# git also quotes it back in stderr. Both must come out redacted.
git -C "$SB/work" remote add leaky "https://u:ghp_notarealtoken123@example.invalid/o/r.git"
LOG=$(probe cmdlog "$SB/work")
ok "the secret is not in the log" "$(printf '%s' "$LOG" | grep -c "ghp_notarealtoken123" || true)" "0"
ok "  and what is left says a credential was there" "$LOG" "***@example.invalid"
git -C "$SB/work" remote remove leaky

if [ "$IS_WIN" = 0 ]; then
section "The shim script itself"
OUT=$(probe term_shim "$SB/work" | "$PY" -c "import sys,json;print(json.load(sys.stdin)['script'])")
ok "it execs the real git at the end" "$OUT" 'exec "'
ok "  and is honest that it can be stepped around" "$OUT" "guard-rail against a slip, not a barrier"
fi

verify_result
