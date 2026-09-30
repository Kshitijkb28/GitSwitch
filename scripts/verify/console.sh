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

section "A real shell, in the repository"
OUT=$(term "git rev-parse --abbrev-ref HEAD")
ok "the shell runs and git answers" "$OUT" "main"
OUT=$(term "pwd")
ok "  it starts in the repository that was chosen" "$OUT" "$SB/work"
OUT=$(term "echo \$GITSWITCH_TERMINAL")
ok "  and says it is GitSwitch's terminal, so a shell profile can tell" "$OUT" "1"

section "The shim is the git the shell finds"
OUT=$(term "command -v git")
ok "git resolves to the shim, not straight to the system one" "$OUT" "terminal-shim"
OUT=$(term "git status --short --branch")
ok "  an ordinary command passes straight through" "$OUT" "## main"
OUT=$(term "git log --oneline")
ok "  and so does a read of history" "$OUT" "first"

section "The documented push-lock bypass is refused"
# push_lock::caveats() names GIT_CONFIG_NOSYSTEM as the one-line way past a
# lock. Inside this app's own terminal that has to be refused, or the app
# hands out the bypass to the thing it promises needs an admin password.
OUT=$(term "GIT_CONFIG_NOSYSTEM=1 git push origin main")
ok "a push that would skip system config is refused" "$OUT" "would skip the system git config"
ok "  and the refusal names the variable to unset" "$OUT" "GIT_CONFIG_NOSYSTEM"
OUT=$(term "GIT_CONFIG_SYSTEM=/dev/null git push origin main")
ok "the other spelling is refused too" "$OUT" "would skip the system git config"
# It must never tell somebody how to turn a lock off.
ok "  and never prints a way to disable the lock" "$(term "GIT_CONFIG_NOSYSTEM=1 git push origin main" | grep -c -- "--uninstall" || true)" "0"
OUT=$(term "GIT_CONFIG_NOSYSTEM=1 git status --short")
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

section "Nothing is left running afterwards"
BEFORE=$(pgrep -f "$SB/work" 2>/dev/null | wc -l | tr -d ' ')
term "echo done" >/dev/null
sleep 1
AFTER=$(pgrep -f "$SB/work" 2>/dev/null | wc -l | tr -d ' ')
ok "closing the terminal leaves no shell behind" "$([ "$AFTER" -le "$BEFORE" ] && echo clean || echo "leaked: $BEFORE -> $AFTER")" "clean"

section "The shim script itself"
OUT=$(probe term_shim "$SB/work" | "$PY" -c "import sys,json;print(json.load(sys.stdin)['script'])")
ok "it execs the real git at the end" "$OUT" 'exec "/usr/bin/git" "$@"'
ok "  and is honest that it can be stepped around" "$OUT" "guard-rail against a slip, not a barrier"

verify_result
