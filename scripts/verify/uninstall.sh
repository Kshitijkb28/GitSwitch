#!/bin/bash
# Uninstalling GitSwitch, end to end against the real backend in a sandbox.
#
# What it would take back is listed first; then the push locks go behind the
# (stand-in) administrator prompt — refused, nothing else changes — then each
# repository's push block, identity guard and gitswitch.* keys (a pre-push
# hook GitSwitch chained to is put back), GitSwitch's block in ~/.gitconfig,
# its signers in allowed_signers and its kept sign-ins, leaving every line the
# person wrote. Last, the real removal script: it waits for the app's process
# to end, deletes GitSwitch's folders (or keeps them) and moves a stand-in
# GitSwitch.app to the Trash.
set -u
source "$(dirname "$0")/lib.sh"
verify_init uninstall "Uninstall Tester" "un@example.test"
SB="$(realdir "$SB")"
if command -v cygpath >/dev/null 2>&1; then
  echo "uninstall suite: skipped on Windows (the profile store does not follow HOME there)"
  verify_result
  exit $?
fi
unset XDG_DATA_HOME XDG_CONFIG_HOME XDG_CACHE_HOME
H="$(realdir "$HOME")"; export HOME="$H"
# The push locks live under a test root, never the real system's; the
# stand-in prompt is chosen per call.
unset GIT_CONFIG_NOSYSTEM
mkdir -p "$SB/root/etc"
export GITSWITCH_LOCK_ROOT="$SB/root" GIT_CONFIG_SYSTEM="$SB/root/etc/gitconfig" GITSWITCH_ELEVATE=cancelled
# The removal script and its log go to the sandbox's temp folder.
mkdir -p "$SB/tmp"; export TMPDIR="$SB/tmp"
case "$(uname -s)" in Darwin) DATA="$H/Library/Application Support/com.gitswitch.app";; *) DATA="$H/.local/share/com.gitswitch.app";; esac

v() { "$PY" -c "import sys,json; d=json.load(sys.stdin); d=d.get('value',d); print(eval(sys.argv[1], {}, {'v': d}))" "$1"; }
args() { "$PY" -c 'import json,sys; print(json.dumps(dict(zip(sys.argv[1::2], [json.loads(x) for x in sys.argv[2::2]]))))' "$@"; }
gcfg() { git -C "$1" config --local --get-regexp '^(gitswitch\.|url\.gitswitch-push-blocked)' 2>/dev/null | wc -l | tr -d ' '; }

section "What uninstalling would take back"
WORK="$H/work"; mkdir -p "$WORK" "$H/.ssh"
ssh-keygen -q -t ed25519 -N "" -C un@example.test -f "$H/.ssh/id_work"
printf 'mine@example.test ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMine\n' > "$H/.ssh/allowed_signers"
printf '[alias]\n\tst = status\n' >> "$H/.gitconfig"
printf '%s' "$("$PY" -c 'import json,sys; print(json.dumps({"name":"Work","git_name":"Me","git_email":"me@work.test","directories":[sys.argv[1]],"ssh_key_path":sys.argv[2],"signing_enabled":True}))' "$WORK" "$H/.ssh/id_work")" > "$SB/p.json"
PID=$(probe profile_apply "$SB" "$SB/p.json" | jqf id)
ok "(a Work profile that signs, over ~/work)" "$([ -n "$PID" ] && echo made)" "made"
git init -q --bare "$SB/remote.git"
for r in guarded plain locked; do git clone -q "$SB/remote.git" "$WORK/$r" 2>/dev/null; done
A="$WORK/guarded"; B="$WORK/plain"; C="$WORK/locked"
printf '#!/bin/sh\necho "the LFS hook ran"\n' > "$A/.git/hooks/pre-push"; chmod +x "$A/.git/hooks/pre-push"
probe push_mode "$A" "guardrail" >/dev/null
pinvoke install_commit_guard "$(args repoPath "\"$A\"" expectedEmail '"me@work.test"')" >/dev/null
git -C "$A" config gitswitch.portOffset 3
ok "(repo A: blocked over its own pre-push hook, an identity guard, a workspace key)" "$(grep -q 'GitSwitch push guard' "$A/.git/hooks/pre-push" && echo guard) $([ -f "$A/.git/hooks/pre-push.gitswitch-saved" ] && echo saved) $(grep -q 'GitSwitch identity guard' "$A/.git/hooks/pre-commit" && echo guard)" "guard saved guard"
GITSWITCH_ELEVATE=direct probe push_mode "$C" "locked" >/dev/null
ok "(repo C: locked)" "$(probe lock_helper_status "$C" | v "len(v['locks'])")" "1"
mkdir -p "$SB/tokens"; printf 'tok' > "$SB/tokens/github_tok-me"; printf '[{"login":"tok-me","id":1}]' > "$SB/tokens/github-signins.json"
B_BEFORE="$(cd "$B" && find .git -type f ! -name index -exec sha256sum {} + 2>/dev/null || find .git -type f ! -name index -exec shasum -a 256 {} +)"

PLAN=$(pinvoke uninstall_plan '{}')
ok "the repositories holding GitSwitch's pieces: A and C, never B" "$(printf '%s' "$PLAN" | v "sorted(p.rsplit('/',1)[-1] for p in v['repos'])")" "['guarded', 'locked']"
ok "the lock and its helper" "$(printf '%s' "$PLAN" | v "(v['lock_helper'], len(v['locks']))")" "(True, 1)"
ok "~/.gitconfig and allowed_signers" "$(printf '%s' "$PLAN" | v "(v['gitconfig'].endswith('/.gitconfig'), v['signers'].endswith('/.ssh/allowed_signers'))")" "(True, True)"
ok "the kept sign-in" "$(printf '%s' "$PLAN" | v "v['signins']")" "['tok-me']"
ok "GitSwitch's data folder" "$(printf '%s' "$PLAN" | v "any(d.endswith('com.gitswitch.app') for d in v['data'])")" "True"
ok "  and only folders named after GitSwitch" "$(printf '%s' "$PLAN" | v "all('com.gitswitch.app' in d.rsplit('/',1)[-1] for d in v['data'])")" "True"
ok "a development build is not removed blindly: it says what to do instead" "$(printf '%s' "$PLAN" | v "v['app']['kind']")" "manual"

section "The administrator prompt refused: nothing is uninstalled"
OUT=$(pinvoke uninstall_prepare '{"keepData":false}')
ok "stopped" "$(printf '%s' "$OUT" | v "v['outcome']")" "stopped"
ok "  saying the locks were not removed" "$(printf '%s' "$OUT" | v "v['message']")" "push locks were not removed"
ok "  ~/.gitconfig, repo A and the sign-in untouched" "$(grep -c 'GitSwitch managed' "$H/.gitconfig") $(gcfg "$A") $([ -f "$SB/tokens/github_tok-me" ] && echo kept)" "2 "
ok "  (the token still there)" "$([ -f "$SB/tokens/github_tok-me" ] && echo kept)" "kept"

section "Allowed: everything GitSwitch wrote is taken back, and nothing else"
BACKUPS=$(ls -a "$H" | grep -c '^\.gitconfig\.backup\.')
OUT=$(GITSWITCH_ELEVATE=direct pinvoke uninstall_prepare '{"keepData":false}')
ok "done, no problems" "$(printf '%s' "$OUT" | v "(v['outcome'], v['problems'])")" "('done', [])"
ok "  the report names the locks, the repositories, the gitconfig, the signers and the sign-in" "$(printf '%s' "$OUT" | v "len(v['done'])")" "5"
ok "the lock is gone" "$(probe lock_helper_status "$C" | v "(len(v['locks']), v['installed'])")" "(0, False)"
ok "repo A: no gitswitch keys or rewrite left" "$(gcfg "$A")" "0"
ok "  its own pre-push hook is back, exactly" "$(cat "$A/.git/hooks/pre-push"; [ -e "$A/.git/hooks/pre-push.gitswitch-saved" ] && echo LEFTOVER)" 'echo "the LFS hook ran"'
ok "  no identity guard" "$([ -e "$A/.git/hooks/pre-commit" ] && echo there || echo gone)" "gone"
ok "repo C: nothing of the lock or block left" "$(gcfg "$C") $(grep -c 'GitSwitch' "$C/.git/hooks/pre-push" 2>/dev/null || echo 0)" "0 0"
ok "repo B: not a byte changed" "$([ "$B_BEFORE" = "$(cd "$B" && find .git -type f ! -name index -exec sha256sum {} + 2>/dev/null || find .git -type f ! -name index -exec shasum -a 256 {} +)" ] && echo same)" "same"
ok "~/.gitconfig: GitSwitch's block gone" "$(grep -c 'GitSwitch managed\|includeIf' "$H/.gitconfig")" "0"
ok "  the person's own lines stay" "$(git config --file "$H/.gitconfig" alias.st) $(git config --file "$H/.gitconfig" user.email)" "status un@example.test"
ok "  a new backup is beside it, holding the block" "$(( $(ls -a "$H" | grep -c '^\.gitconfig\.backup\.') - BACKUPS )) $(grep -l 'GitSwitch managed' "$H"/.gitconfig.backup.* | wc -l | tr -d ' ')" "1 "
ok "allowed_signers: only the person's own line" "$(cat "$H/.ssh/allowed_signers")" "mine@example.test ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMine"
ok "  (and nothing else)" "$(wc -l < "$H/.ssh/allowed_signers" | tr -d ' ')" "1"
ok "the sign-in's token is gone" "$([ -e "$SB/tokens/github_tok-me" ] && echo there || echo gone) $(cat "$SB/tokens/github-signins.json")" "gone []"
ok "the SSH key stays" "$([ -f "$H/.ssh/id_work" ] && [ -f "$H/.ssh/id_work.pub" ] && echo kept)" "kept"
ok "git in repo A still works as the person's own" "$(git -C "$A" config user.email)" "un@example.test"

section "Twice: nothing left to do"
OUT=$(pinvoke uninstall_prepare '{"keepData":false}')
ok "done with nothing to report" "$(printf '%s' "$OUT" | v "(v['outcome'], v['done'], v['problems'])")" "('done', [], [])"
PLAN=$(pinvoke uninstall_plan '{}')
ok "the plan lists nothing more to take back" "$(printf '%s' "$PLAN" | v "(v['repos'], v['gitconfig'], v['signers'], v['lock_helper'], v['locks'], v['signins'])")" "([], None, None, False, [], [])"

section "The removal script, after the app has quit"
ended() { sh -c 'echo $$'; }
bundle() { mkdir -p "$1/Contents/MacOS"; printf 'app' > "$1/Contents/MacOS/gitswitch"; }
waitlog() { for _ in $(seq 1 100); do [ "$(grep -c '^done' "$SB/tmp/gitswitch-uninstall.log" 2>/dev/null)" -ge "$1" ] && return 0; sleep 0.1; done; return 1; }
mkdir -p "$DATA/gitconfigs" "$H/.Trash"
bundle "$SB/Apps/GitSwitch.app"
OUT=$(pinvoke uninstall_start_remover "$(args bundle "\"$SB/Apps/GitSwitch.app\"" pid "$(ended)" keep_data true)")
ok "it starts, and says where it writes what it did" "$(printf '%s' "$OUT" | v "v.endswith('gitswitch-uninstall.log')")" "True"
waitlog 1; ok "  it finishes" "$(grep -c '^done' "$SB/tmp/gitswitch-uninstall.log")" "1"
ok "the app is in the Trash, not deleted" "$([ -e "$SB/Apps/GitSwitch.app" ] && echo still-there) $([ -f "$H/.Trash/GitSwitch.app/Contents/MacOS/gitswitch" ] && echo in-trash)" " in-trash"
ok "kept data: GitSwitch's folder stays" "$([ -d "$DATA/gitconfigs" ] && echo kept)" "kept"
ok "  and the script removed itself" "$(ls "$SB/tmp" | grep -c '\.sh$')" "0"
bundle "$SB/Apps/GitSwitch.app"
pinvoke uninstall_start_remover "$(args bundle "\"$SB/Apps/GitSwitch.app\"" pid "$(ended)" keep_data false)" >/dev/null
waitlog 2; ok "a second time: it finishes" "$(grep -c '^done' "$SB/tmp/gitswitch-uninstall.log")" "2"
ok "  the first copy in the Trash is not overwritten: the second gets its own name" "$(ls "$H/.Trash" | grep -c '^GitSwitch.*\.app$')" "2"
ok "  GitSwitch's data folder is gone" "$([ -e "$DATA" ] && echo still-there || echo gone)" "gone"
ok "  the rest of the home folder is untouched" "$([ -f "$H/.ssh/id_work" ] && [ -d "$WORK/guarded/.git" ] && [ -f "$H/.gitconfig" ] && echo intact)" "intact"
sleep 1 & SLEEPER=$!
bundle "$SB/Apps/Late.app"
pinvoke uninstall_start_remover "$(args bundle "\"$SB/Apps/Late.app\"" pid "$SLEEPER" keep_data true)" >/dev/null
sleep 0.4
ok "while the app still runs, nothing is removed" "$([ -e "$SB/Apps/Late.app" ] && echo waiting)" "waiting"
wait "$SLEEPER"; waitlog 3
ok "  once it has quit, the app goes" "$([ -e "$SB/Apps/Late.app" ] && echo still-there || echo gone) $([ -e "$H/.Trash/Late.app" ] && echo in-trash)" "gone in-trash"

verify_result
