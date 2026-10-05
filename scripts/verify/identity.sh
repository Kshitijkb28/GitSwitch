#!/bin/bash
# Identity suite: what the app writes into ~/.gitconfig is what git reads back.
#
# Every value a profile holds passes through git's config syntax, where `#`
# and `;` start a comment, `"` and `\` are special, and a space in a key path
# splits the ssh command in two. The profiles here carry all of those. They
# are written into the sandbox HOME that lib.sh set up; the real one is never
# touched.
set -u
source "$(dirname "$0")/lib.sh"
verify_init identity "Identity Tester" "identity@example.test"
SB="$(realdir "$SB")"
# The profile store lives in the platform data dir, which follows HOME on
# macOS and Linux but not on Windows — there this suite would write into the
# real store, so it only runs where that cannot happen.
if command -v cygpath >/dev/null 2>&1; then
  echo "identity suite: skipped on Windows (the profile store does not follow HOME there)"
  verify_result
fi
unset XDG_DATA_HOME XDG_CONFIG_HOME

# profile_json <file> <name> <git_name> <git_email> <key-or-empty> [dir...]
profile_json() {
  "$PY" - "$@" <<'PY'
import json, sys
f, name, gname, email, key, *dirs = sys.argv[1:]
p = {"name": name, "git_name": gname, "git_email": email, "directories": dirs}
if key: p["ssh_key_path"] = key
json.dump(p, open(f, "w"))
PY
}

section "The default profile, with every character git's syntax treats specially"
NAME1='Ann #1 "The Hammer" O'"'"'Brien'
EMAIL1='ann;tag@example.test'
profile_json "$SB/p1.json" "Personal" "$NAME1" "$EMAIL1" ""
OUT=$(probe profile_apply "$SB" "$SB/p1.json")
ok "the first profile becomes the default" "$(printf '%s' "$OUT" | jqf is_default)" "True"
ok "git can still read the whole config" "$(git config --global --list >/dev/null 2>&1 && echo readable || echo BROKEN)" "readable"
ok "the name comes back whole: hash, quotes and apostrophe" "$(git config --global user.name)" "$NAME1"
ok "  so does an email with a semicolon" "$(git config --global user.email)" "$EMAIL1"
ok "  the managed block is marked as such" "$(cat "$HOME/.gitconfig")" ">>> GitSwitch managed"

section "A folder profile: a folder with a space, a quote and a hash; a key with a space"
WORKDIR="$SB/Work \"2024\" #1"; mkdir -p "$WORKDIR"
KEY="$SB/home/.ssh/my key"; mkdir -p "$SB/home/.ssh"; printf 'not a real key\n' > "$KEY"
NAME2='Work Person'; EMAIL2='work@example.test'
profile_json "$SB/p2.json" "Work" "$NAME2" "$EMAIL2" "$KEY" "$WORKDIR"
OUT=$(probe profile_apply "$SB" "$SB/p2.json")
ok "the profile is created" "$(printf '%s' "$OUT" | jqf name)" "Work"
git init -q "$WORKDIR/repo"
ok "git can still read the whole config, include and all" "$(git -C "$WORKDIR/repo" config --list >/dev/null 2>&1 && echo readable || echo BROKEN)" "readable"
ok "inside the folder, git is the folder profile" "$(git -C "$WORKDIR/repo" config user.email)" "$EMAIL2"
ok "  with its ssh command, the key path quoted for the shell" "$(git -C "$WORKDIR/repo" config core.sshCommand)" "ssh -i '$KEY' -o IdentitiesOnly=yes"
git -C "$WORKDIR/repo" commit -q --allow-empty -m x
ok "  a real commit carries that identity" "$(git -C "$WORKDIR/repo" log -1 --format='%an <%ae>')" "$NAME2 <$EMAIL2>"
git init -q "$SB/elsewhere"
ok "outside it, the default profile applies" "$(git -C "$SB/elsewhere" config user.name)" "$NAME1"
git -C "$SB/elsewhere" commit -q --allow-empty -m x
ok "  down to the commit" "$(git -C "$SB/elsewhere" log -1 --format='%an <%ae>')" "$NAME1 <$EMAIL1>"
ok "  with no ssh command of its own" "$(git -C "$SB/elsewhere" config core.sshCommand || echo none)" "none"

section "A remote rule follows the repository wherever it is cloned"
# git >= 2.36 only — the Doctor says so for older gits; here the suite does.
GITV="$(git --version | awk '{print $3}')"
if [ "$(printf '%s\n2.36.0\n' "$GITV" | sort -V | head -1)" = "2.36.0" ]; then
NAME3='Acme Person'; EMAIL3='acme@example.test'
"$PY" - "$SB/p3.json" "$NAME3" "$EMAIL3" <<'JSONPY'
import json, sys
f, gname, email = sys.argv[1:]
json.dump({"name": "Acme", "git_name": gname, "git_email": email, "directories": [], "remote_patterns": ["acme/", "Other/Exact"], "github_login": "acme-bot"}, open(f, "w"))
JSONPY
OUT=$(probe profile_apply "$SB" "$SB/p3.json")
ok "a profile can claim remotes, with no folder at all" "$(printf '%s' "$OUT" | jqf remote_patterns)" '["acme/", "Other/Exact"]'
ok "  git can still read the whole config" "$(git config --global --list >/dev/null 2>&1 && echo readable || echo BROKEN)" "readable"
clone_with_remote() { git init -q "$1"; git -C "$1" remote add origin "$2"; }
clone_with_remote "$SB/anywhere/https" "https://github.com/acme/thing.git"
ok "an HTTPS clone of that owner, in no profile folder, commits as the profile" "$(git -C "$SB/anywhere/https" config user.email)" "$EMAIL3"
clone_with_remote "$SB/anywhere/ssh" "git@github.com:acme/other.git"
ok "  so does the scp-style SSH spelling" "$(git -C "$SB/anywhere/ssh" config user.email)" "$EMAIL3"
clone_with_remote "$SB/anywhere/sshurl" "ssh://git@github.com/acme/other.git"
ok "  and the ssh:// spelling" "$(git -C "$SB/anywhere/sshurl" config user.email)" "$EMAIL3"
clone_with_remote "$SB/anywhere/case" "https://github.com/ACME/Thing"
ok "  owner case does not matter, as on GitHub" "$(git -C "$SB/anywhere/case" config user.email)" "$EMAIL3"
clone_with_remote "$SB/anywhere/exact" "https://github.com/other/exact.git"
ok "an exact owner/repo rule matches that repository" "$(git -C "$SB/anywhere/exact" config user.email)" "$EMAIL3"
clone_with_remote "$SB/anywhere/sibling" "https://github.com/other/exact2.git"
ok "  but not its neighbour" "$(git -C "$SB/anywhere/sibling" config user.email)" "$EMAIL1"
clone_with_remote "$SB/anywhere/unrelated" "https://github.com/someone/else.git"
ok "an unrelated remote still gets the default profile" "$(git -C "$SB/anywhere/unrelated" config user.email)" "$EMAIL1"
git -C "$SB/anywhere/https" commit -q --allow-empty -m x
ok "  a real commit carries the remote profile's identity" "$(git -C "$SB/anywhere/https" log -1 --format='%an <%ae>')" "$NAME3 <$EMAIL3>"
clone_with_remote "$WORKDIR/acme-clone" "https://github.com/acme/thing.git"
ok "inside a folder profile's folder, the folder rule wins over the remote rule" "$(git -C "$WORKDIR/acme-clone" config user.email)" "$EMAIL2"
# The app ties such a clone back to its profile through where git read the email from.
pinvoke() { PROBE_OP=invoke PROBE_CMD="$1" PROBE_JSON="$2" "$BIN" probe --ignored --nocapture 2>/dev/null | sed -n 's/^PROBE_OUT //p'; }
ST=$(pinvoke changes_repo_status "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1]}))' "$SB/anywhere/https")")
ok "the Changes page knows which profile a remote-rule clone belongs to" "$(printf '%s' "$ST" | jqf value.identity.profile_name)" "Acme"
WHO=$(GITSWITCH_GITHUB_API="http://127.0.0.1:9" pinvoke changes_push_identity "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1],"refresh":False}))' "$SB/anywhere/https")")
ok "  and which account its pushes are expected to go as" "$(printf '%s' "$WHO" | jqf value.expected_login)" "acme-bot"
else
  echo "  (git $GITV is older than 2.36: remote rules are not available here; skipped)"
fi

section "Doctor sees a repository-local override and can remove it"
# `probe invoke` runs the same #[tauri::command] the Doctor page calls.
pinvoke() { PROBE_OP=invoke PROBE_CMD="$1" PROBE_JSON="$2" "$BIN" probe --ignored --nocapture 2>/dev/null | sed -n 's/^PROBE_OUT //p'; }
git -C "$WORKDIR/repo" config user.email someone-else@example.test
ok "git now answers with the override" "$(git -C "$WORKDIR/repo" config user.email)" "someone-else@example.test"
D=$(pinvoke doctor_check_repos '{}')
ok "the Doctor reports it" "$D" "local-override"
ok "  naming the key, the value and the profile it defeats" "$D" "user.email = someone-else@example.test"
ok "  and which profile" "$D" 'profile \"Work\"'
ok "  with a one-click repair" "$D" "unset-local:user.email:"
git -C "$WORKDIR/repo" config user.name "$NAME2"
ok "a local value equal to the profile's is not reported" "$(pinvoke doctor_check_repos '{}' | grep -c 'overrides user.name' || true)" "0"
F=$(pinvoke doctor_unset_local "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1],"key":"user.email"}))' "$WORKDIR/repo")")
ok "the repair says what it did" "$F" "Removed user.email"
ok "  and git answers with the profile again" "$(git -C "$WORKDIR/repo" config user.email)" "$EMAIL2"
ok "  so the finding is gone" "$(pinvoke doctor_check_repos '{}' | grep -c local-override || true)" "0"
ok "a key GitSwitch does not manage is refused" "$(pinvoke doctor_unset_local "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1],"key":"core.editor"}))' "$WORKDIR/repo")")" "not an identity setting"
git -C "$WORKDIR/repo" config --unset user.name

section "Doctor names an ssh alias behind a remote, and which key it means"
printf 'Host gh-work\n  HostName github.com\n  IdentityFile "%s"\n' "$KEY" > "$SB/sshcfg"
git -C "$WORKDIR/repo" remote add origin git@gh-work:acme/thing.git
D=$(GITSWITCH_SSH_CONFIG="$SB/sshcfg" pinvoke doctor_check_repos '{}')
ok "the alias is reported with the host it stands for" "$D" "ssh-alias"
ok "  naming github.com" "$D" "github.com (ssh -G)"
ok "  and that the profile's own key still wins" "$D" "profile's key wins"
git -C "$WORKDIR/repo" remote remove origin
printf 'Host github.com-work\n  HostName github.com\n  IdentityFile "%s"\n' "$KEY" >> "$SB/sshcfg"
git -C "$WORKDIR/repo" remote add origin git@github.com-work:acme/thing.git
D=$(GITSWITCH_SSH_CONFIG="$SB/sshcfg" pinvoke doctor_check_repos '{}')
ok "a dotted alias (github.com-work) is a finding too" "$D" "ssh-alias"
git -C "$WORKDIR/repo" remote remove origin

section "What would corrupt a commit is refused before it reaches any config"
BEFORE="$(sha256file "$HOME/.gitconfig")"
profile_json "$SB/p3.json" "Bad" "$(printf 'Evil\nName')" "e@example.test" ""
ok "a line break in a name is refused" "$(probe profile_apply "$SB" "$SB/p3.json")" "line breaks"
profile_json "$SB/p4.json" "Bad" "Fine Name" "two words@example.test" ""
ok "a space in an email is refused" "$(probe profile_apply "$SB" "$SB/p4.json")" "spaces"
profile_json "$SB/p5.json" "Bad" "Fine Name" "<e@example.test>" ""
ok "angle brackets in an email are refused" "$(probe profile_apply "$SB" "$SB/p5.json")" "angle brackets"
mkdir -p "$SB/[archive]"
profile_json "$SB/p6.json" "Bad" "Fine Name" "e@example.test" "" "$SB/[archive]"
ok "a folder git could only read as a wildcard is refused" "$(probe profile_apply "$SB" "$SB/p6.json")" "wildcard"
profile_json "$SB/p7.json" "Bad" "Fine Name" "e@example.test" "" "relative/dir"
ok "a relative folder is refused" "$(probe profile_apply "$SB" "$SB/p7.json")" "full path"
ok "  and the config file is byte-for-byte as it was" "$(sha256file "$HOME/.gitconfig")" "$BEFORE"
ok "  still readable" "$(git config --global --list >/dev/null 2>&1 && echo readable || echo BROKEN)" "readable"

verify_result
