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
OUT=$(probe profile_apply "$SB" "$SB/p2.json"); WID="$(printf '%s' "$OUT" | jqf id)"
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
# The VS Code extension decides the same way, from the same config: its own
# resolver (vscode-extension/src/resolve.ts), run by bun.
ext() {
  if command -v bun >/dev/null 2>&1; then bun "$VERIFY_DIR/harness/ext-identity.ts" "$1" 2>&1
  else echo '{"error":"bun is not installed: the VS Code extension resolver cannot run"}'; fi
}
ok "the VS Code extension names a remote-rule clone's profile" "$(ext "$SB/anywhere/https" | jqf profile)" "Acme"
ok "  and raises no mismatch for it" "$(ext "$SB/anywhere/https" | jqf mismatch)" "False"
git -C "$SB/anywhere/https" config user.email ACME@EXAMPLE.TEST
ok "  a local override that differs only in letter case is no mismatch" "$(ext "$SB/anywhere/https" | jqf mismatch)" "False"
git -C "$SB/anywhere/https" config --unset user.email
ok "  inside a folder profile's folder, the folder profile" "$(ext "$WORKDIR/acme-clone" | jqf profile)" "Work"
ok "  an unrelated clone: the default profile, no mismatch" "$(ext "$SB/anywhere/unrelated" | jqf profile) $(ext "$SB/anywhere/unrelated" | jqf mismatch)" "Personal False"
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

section "What is on this Mac is read afresh: a clone deleted, moved or made behind the app's back"
for r in alpha beta; do git init -q "$WORKDIR/$r"; git -C "$WORKDIR/$r" remote add origin "git@github.com:behindback/$r.git"; done
# The folder holds quotes and a hash on purpose: it reaches the script through
# the environment, never pasted into its text.
cat > "$SB/behind-its-back.sh" <<'EOF'
rm -rf "$WD/alpha"
mv "$WD/beta" "$WD/beta-moved"
git init -q "$WD/gamma" && git -C "$WD/gamma" remote add origin git@github.com:behindback/gamma.git
EOF
OUT=$(WD="$WORKDIR" probe local_clones_around "" "$SB/behind-its-back.sh")
clones() { printf '%s' "$OUT" | "$PY" -c "import sys,json;d=json.load(sys.stdin)['$1'];print(sorted((k, v.rsplit('/',1)[-1]) for k,v in d.items() if k.startswith('behindback/')))"; }
ok "(both clones are seen first)" "$(clones before)" "[('behindback/alpha', 'alpha'), ('behindback/beta', 'beta')]"
ok "a clone deleted behind the app's back is gone from the very next read, a moved one is found where it is, and a new one is in it" "$(clones after)" "[('behindback/beta', 'beta-moved'), ('behindback/gamma', 'gamma')]"

section "A repository in a profile folder inside another profile's folder is listed as the inner profile's"
INNERDIR="$WORKDIR/inner"; mkdir -p "$INNERDIR"
profile_json "$SB/p8.json" "Inner" "Inner Person" "inner@example.test" "" "$INNERDIR"
OUT=$(probe profile_apply "$SB" "$SB/p8.json")
git init -q "$INNERDIR/api"
ok "(git itself gives that repository the inner profile's identity)" "$(git -C "$INNERDIR/api" config user.email)" "inner@example.test"
LISTED=$(pinvoke history_list_repos "{}")
ok "the History account filter lists it under the inner profile too" "$(printf '%s' "$LISTED" | "$PY" -c 'import sys,json;print([r["profile_name"] for r in json.load(sys.stdin)["value"] if r["path"].endswith("/inner/api")])')" "['Inner']"

section "A profile with pushes turned off stops a push to any remote in its folders"
# Whatever the address: an organisation's SSH-certificate form, a user in an
# HTTPS URL, an ssh alias, another host, a local path. Nothing here reaches a
# network: only the local one is pushed to or fetched from.
NOPUSH="$SB/NoPush"; mkdir -p "$NOPUSH"; git init -q --bare "$SB/target.git"
np_repo() { git init -q "$NOPUSH/$1"; git -C "$NOPUSH/$1" commit -q --allow-empty -m x; git -C "$NOPUSH/$1" remote add origin "$2"; }
np_repo org "org-1234567@github.com:blockorg/w.git"
np_repo userhttps "https://someone@github.com/blockorg/w.git"
np_repo alias "gh-work:blockorg/w.git"
np_repo otherhost "git@gitlab.example.com:blockorg/w.git"
np_repo local "$SB/target.git"
"$PY" - "$SB/pnopush.json" "$NOPUSH" <<'JSONPY'
import json, sys
f, folder = sys.argv[1:]
json.dump({"name": "NoPush", "git_name": "No Push", "git_email": "nopush@example.test", "directories": [folder], "allow_push": False}, open(f, "w"))
JSONPY
OUT=$(probe profile_apply "$SB" "$SB/pnopush.json")
ok "(a profile with pushes turned off is created)" "$(printf '%s' "$OUT" | jqf allow_push)" "False"
for r in org userhttps alias otherhost local; do
  ok "the push address of the $r remote is rewritten" "$(git -C "$NOPUSH/$r" remote get-url --push origin)" "gitswitch-push-blocked://"
done
ok "a push from a terminal fails" "$(git -C "$NOPUSH/local" push origin HEAD:refs/heads/main 2>&1)" "gitswitch-push-blocked"
ok "  and nothing reached the repository" "$(git -C "$SB/target.git" rev-parse -q --verify refs/heads/main || echo none)" "none"
ok "  fetch still works" "$(git -C "$NOPUSH/local" fetch origin 2>&1 && echo fetched)" "fetched"
ok "  and the folder's identity still applies" "$(git -C "$NOPUSH/org" config user.email)" "nopush@example.test"
ST=$(pinvoke changes_push_state "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1]}))' "$NOPUSH/org")")
ok "the Repository page finds no remote the block leaves open" "$(printf '%s' "$ST" | "$PY" -c 'import sys,json;v=json.load(sys.stdin)["value"];print(v["profile_blocked"], [g for g in v.get("gaps", []) if "stopped by the URL rewrite" in g])')" "True []"
WHERE="$("$PY" -c 'import os;h=os.environ["HOME"];c=[os.path.join(h,"Library/Application Support/com.gitswitch.app/gitconfigs"),os.path.join(h,".local/share/com.gitswitch.app/gitconfigs")];print(next((p for p in c if os.path.isdir(p)),"MISSING"))')"
for f in "$WHERE"/*.gitconfig; do grep -q 'pushInsteadOf = ""' "$f" && sed -i.bak '/pushInsteadOf = ""/d' "$f" && rm -f "$f.bak"; done
ok "(an include file an older build wrote has no catch-all)" "$(git -C "$NOPUSH/org" remote get-url --push origin)" "org-1234567@github.com:blockorg/w.git"
probe refresh_profile_includes "$SB" >/dev/null
ok "the app's start-up refresh gives it this build's block, without the profile being saved again" "$(git -C "$NOPUSH/org" remote get-url --push origin)" "gitswitch-push-blocked://"

section "A folder typed in another letter case, or named through a link, still gets its profile"
# git matches a folder rule letter for letter against the REAL path of .git;
# macOS and Windows disks ignore letter case, so a folder typed otherwise
# opens fine and never matches.
mkdir -p "$SB/CaseProbe"
if [ -d "$SB/caseprobe" ]; then
  CASED="$SB/Cased Work"; mkdir -p "$CASED"; git init -q "$CASED/repo"
  profile_json "$SB/pcase.json" "Cased" "Cased Person" "cased@example.test" "" "$SB/cased work"
  OUT=$(probe profile_apply "$SB" "$SB/pcase.json")
  CASED_ID="$(printf '%s' "$OUT" | jqf id)"
  ok "a folder typed in another letter case is stored as the disk spells it" "$(printf '%s' "$OUT" | jqf directories)" "[\"$CASED\"]"
  ok "  and git gives its repositories that profile" "$(git -C "$CASED/repo" config user.email)" "cased@example.test"
else
  echo "  (this disk tells letter case apart: a folder typed otherwise is another folder)"
fi
MOVE="$SB/Move Here"; mkdir -p "$MOVE"; git init -q "$MOVE/repo"
profile_json "$SB/pm1.json" "First" "First Person" "first@example.test" "" "$MOVE"
OUT=$(probe profile_apply "$SB" "$SB/pm1.json"); FIRST_ID="$(printf '%s' "$OUT" | jqf id)"
ok "(the folder is First's)" "$(git -C "$MOVE/repo" config user.email)" "first@example.test"
ln -s "$MOVE" "$SB/move-link"
profile_json "$SB/pm2.json" "Second" "Second Person" "second@example.test" "" "$SB/move-link"
OUT=$(probe profile_apply "$SB" "$SB/pm2.json")
ok "named through a link, the folder moves to the profile that claims it" "$(git -C "$MOVE/repo" config user.email)" "second@example.test"
ok "  also for git reached through the link" "$(git -C "$SB/move-link/repo" config user.email)" "second@example.test"
ok "  the link keeps its name in the profile" "$(printf '%s' "$OUT" | jqf directories)" "[\"$SB/move-link\"]"
holders() { pinvoke get_profiles '{}' | "$PY" -c 'import sys,os,json;print(sorted(p["name"] for p in json.load(sys.stdin)["value"] if any(os.path.exists(d) and os.path.samefile(d, sys.argv[1]) for d in p["directories"])))' "$MOVE"; }
ok "  and only one profile lists it" "$(holders)" "['Second']"

section "Doctor: a folder spelled otherwise than the disk does, and one folder named two ways"
# Hand-edited stores (or folders saved before the spelling was corrected).
STORE="$("$PY" -c 'import os;h=os.environ["HOME"];c=[os.path.join(h,"Library/Application Support/com.gitswitch.app/profiles.json"),os.path.join(h,".local/share/com.gitswitch.app/profiles.json")];print(next((p for p in c if os.path.exists(p)),"MISSING"))')"
ok "(the sandbox's profile store is found)" "$([ -f "$STORE" ] && echo found || echo "$STORE")" "found"
edit_store() { "$PY" - "$STORE" "$1" "$2" <<'PY'
import json, sys
f, pid, folder = sys.argv[1:]
d = json.load(open(f))
for p in d["profiles"]:
    if p["id"] == pid:
        p["directories"].append(folder)
json.dump(d, open(f, "w"))
PY
}
edit_store "$FIRST_ID" "$SB/Move Here"
DOC=$(pinvoke doctor_check_profiles '{}')
ok "Doctor calls one folder named two ways a conflict" "$(printf '%s' "$DOC" | "$PY" -c 'import sys,json;print([f["detail"] for f in json.load(sys.stdin)["value"] if f["id"]=="folder-conflict"])')" "are the same folder"
if [ -n "${CASED_ID:-}" ]; then
  "$PY" - "$STORE" "$CASED_ID" "$SB/cased work" <<'PY'
import json, sys
f, pid, folder = sys.argv[1:]
d = json.load(open(f))
for p in d["profiles"]:
    if p["id"] == pid:
        p["directories"] = [folder]
json.dump(d, open(f, "w"))
PY
  pinvoke apply_git_config '{}' >/dev/null
  ok "a folder stored in another letter case still gets its profile once git's rules are written: the disk's spelling is written beside it" "$(git -C "$CASED/repo" config user.email)" "cased@example.test"
  DOC=$(pinvoke doctor_check_profiles '{}')
  ok "Doctor names a folder spelled otherwise than the disk does" "$(printf '%s' "$DOC" | "$PY" -c 'import sys,json;print([(f["detail"], f["fix"]) for f in json.load(sys.stdin)["value"] if f["id"]=="folder-spelling"])')" "is $SB/Cased Work on disk"
  printf '{"id":"%s","directories":["%s"]}' "$CASED_ID" "$SB/cased work" > "$SB/pfix.json"
  OUT=$(probe profile_update "$SB" "$SB/pfix.json")
  ok "  and its fix, saving the profile again, stores the disk's spelling" "$(printf '%s' "$OUT" | jqf directories)" "[\"$CASED\"]"
  ok "  so git gives the repositories there that profile" "$(git -C "$CASED/repo" config user.email)" "cased@example.test"
  ok "  and Doctor has nothing more to say about it" "$(pinvoke doctor_check_profiles '{}' | "$PY" -c 'import sys,json;print(len([f for f in json.load(sys.stdin)["value"] if f["id"]=="folder-spelling"]))')" "0"
fi

section "The identity guard follows the profile's email"
G="$WORKDIR/guarded"; git init -q "$G"
printf '#!/bin/sh\necho "FOREIGN HOOK RAN" >&2\nexit 0\n' > "$G/.git/hooks/pre-commit"; chmod +x "$G/.git/hooks/pre-commit"
probe guard_install "$G" "$EMAIL2" >/dev/null
ok "(the guard expects the Work profile's email)" "$(probe guard_email "$G")" "$EMAIL2"
printf '{"id":"%s","git_email":"work2@example.test"}' "$WID" > "$SB/pemail.json"
OUT=$(probe profile_update "$SB" "$SB/pemail.json")
C=$(git -C "$G" commit --allow-empty -m "after the email changed" 2>&1)
ok "a guarded repository still commits after its profile's email changes" "$(git -C "$G" log -1 --format=%ae 2>/dev/null || echo "refused: $C")" "work2@example.test"
ok "  the guard expects the new email" "$(probe guard_email "$G")" "work2@example.test"
ok "  the hook that was there before still runs" "$C" "FOREIGN HOOK RAN"
ST=$(pinvoke changes_repo_status "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1]}))' "$G")")
ok "  and the Changes page reads the guard's email" "$(printf '%s' "$ST" | jqf value.identity.guard_email)" "work2@example.test"

section "Removing a GitHub CLI account signs it out of every host it is on"
# A gh that answers `auth status` from a file and records each logout,
# refusing the way gh 2.40+ does.
GHB="$SB/ghbin"; mkdir -p "$GHB"
cat > "$GHB/gh" <<'FAKEGH'
#!/bin/sh
status="$GH_FAKE_DIR/gh-status.txt"
case "$1 $2" in
  "auth status") cat "$status" >&2; exit 0 ;;
  "auth logout")
    echo "$*" >> "$GH_FAKE_DIR/gh-calls.txt"
    if [ -n "${GH_TOKEN:-}" ]; then echo "The value of the GH_TOKEN environment variable is being used for authentication." >&2; exit 1; fi
    shift 2; host=""; user=""
    while [ $# -gt 0 ]; do case "$1" in --hostname) host="$2"; shift 2;; --user) user="$2"; shift 2;; *) shift;; esac; done
    n=$(grep -c "account $user " "$status")
    if [ "$n" = 0 ]; then echo "no accounts matched that criteria" >&2; exit 1; fi
    if [ -z "$host" ] && [ "$n" -gt 1 ]; then echo "unable to determine which account to log out of, please specify --hostname and --user" >&2; exit 1; fi
    if [ -n "$host" ]; then grep -v "to $host account $user " "$status" > "$status.new"; else grep -v "account $user " "$status" > "$status.new"; fi
    mv "$status.new" "$status"; exit 0 ;;
esac
exit 1
FAKEGH
chmod +x "$GHB/gh"
printf '  ✓ Logged in to github.com account alice (keyring)\n  - Active account: true\n  ✓ Logged in to github.com account bob (keyring)\n  ✓ Logged in to ghe.example.com account alice (keyring)\n' > "$SB/gh-status.txt"
: > "$SB/gh-calls.txt"
ghx() { ( unset GH_TOKEN GITHUB_TOKEN; PATH="$GHB:$PATH" GH_FAKE_DIR="$SB" pinvoke "$@" ); }
ok "(gh lists each login once)" "$(ghx gh_list_accounts '{}' | jqf value)" '["alice", "bob"]'
OUT=$(ghx gh_logout '{"account":"alice"}')
ok "an account on two hosts is removed from both" "$(printf '%s' "$OUT" | jqf ok):$(printf '%s' "$OUT" | jqf value)" "True:Removed alice"
ok "  github.com named explicitly" "$(cat "$SB/gh-calls.txt")" "--hostname github.com --user alice"
ok "  and the other host" "$(cat "$SB/gh-calls.txt")" "--hostname ghe.example.com --user alice"
ok "  the other account is still signed in" "$(ghx gh_list_accounts '{}' | jqf value)" '["bob"]'
ok "an account already signed out elsewhere counts as removed" "$(ghx gh_logout '{"account":"carol"}' | jqf value)" "already signed out"
OUT=$( PATH="$GHB:$PATH" GH_FAKE_DIR="$SB" GH_TOKEN=x pinvoke gh_logout '{"account":"bob"}' )
ok "a refusal reaches the person in gh's own words" "$(printf '%s' "$OUT" | jqf error)" "GH_TOKEN"

section "An account's own addresses come back with it, when the token may read them"
"$PY" - "$SB/emails-api.log" > "$SB/emails-api-port" <<'PYAPI' &
import http.server, sys, json
log = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(log, "a") as f:
            f.write(self.path + "\n")
        auth = self.headers.get("Authorization", "")
        if self.path == "/user":
            body = json.dumps({"id": 42, "login": "octo-home", "name": "Octo Home", "email": None, "avatar_url": ""}).encode()
            self.send_response(200)
            self.send_header("X-OAuth-Scopes", "repo, user:email" if auth.endswith("t-scoped") else "repo, read:org")
        elif self.path == "/user/emails":
            body = json.dumps([
                {"email": "old@home.test", "primary": False, "verified": False},
                {"email": "me@home.test", "primary": True, "verified": True},
                {"email": "alt@home.test", "primary": False, "verified": True},
            ]).encode()
            self.send_response(200)
        else:
            body = b"{}"
            self.send_response(404)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
PYAPI
EMAILS_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/emails-api-port" ] && break; sleep 0.5; done
EAPI="http://127.0.0.1:$(cat "$SB/emails-api-port")"
OUT=$(GITSWITCH_GITHUB_API="$EAPI" pinvoke verify_github_token '{"token":"t-scoped"}')
ok "the primary verified address comes back" "$(printf '%s' "$OUT" | jqf value.primary_email)" "me@home.test"
ok "  with every verified address and no unverified one" "$(printf '%s' "$OUT" | jqf value.emails)" '["me@home.test", "alt@home.test"]'
: > "$SB/emails-api.log"
OUT=$(GITSWITCH_GITHUB_API="$EAPI" pinvoke verify_github_token '{"token":"t-plain"}')
ok "a token without user:email says so" "$(printf '%s' "$OUT" | jqf value.email_scope_missing)" "True"
ok "  and spends no request on the addresses" "$(grep -c '^/user/emails' "$SB/emails-api.log" || true)" "0"
{ kill "$EMAILS_PID" && wait "$EMAILS_PID"; } 2>/dev/null

section "Doctor names the gh token scopes an account lacks"
FGH="$SB/fakegh"; mkdir -p "$FGH"
cat > "$FGH/gh" <<'FAKEGH'
#!/bin/sh
case "$1" in --version) echo "gh version 2.60.0"; exit 0;; esac
if [ "$1 $2" = "auth status" ]; then cat >&2 <<'STATUS'
github.com
  ✓ Logged in to github.com account alice-dev (keyring)
  - Active account: true
  - Token scopes: 'gist', 'read:org', 'repo'

  ✓ Logged in to github.com account bob-work (keyring)
  - Active account: false
  - Token scopes: 'read:org', 'repo'

  ✓ Logged in to github.com account dave-full (keyring)
  - Active account: false
  - Token scopes: 'admin:public_key', 'repo', 'write:ssh_signing_key'

  ✓ Logged in to github.com account carol-fg (GH_TOKEN)
  - Active account: false
  - Token: github_pat_************************************
STATUS
exit 0; fi
exit 1
FAKEGH
chmod +x "$FGH/gh"
"$PY" - "$SB/psign.json" <<'JSONPY'
import json, sys
json.dump({"name": "Signer", "git_name": "alice-dev", "git_email": "alice@example.test", "directories": [], "signing_enabled": True, "github_login": "alice-dev"}, open(sys.argv[1], "w"))
JSONPY
probe profile_apply "$SB" "$SB/psign.json" >/dev/null
E=$( PATH="$FGH:$PATH" pinvoke doctor_check_environment '{}' )
ok "an account missing the key scope is named" "$E" "gh-token-scopes:alice-dev"
ok "  with the scope registering keys needs" "$E" "admin:public_key"
ok "  and the signing scope its signing profile needs" "$E" "write:ssh_signing_key"
ok "  with the command that grants them" "$E" "gh auth refresh -h github.com -s admin:public_key,write:ssh_signing_key"
ok "an inactive account is switched to first" "$E" "gh auth switch -h github.com -u bob-work"
ok "an account with every scope is not reported" "$(printf '%s' "$E" | grep -c dave-full || true)" "0"
ok "a token gh lists no scopes for is not reported" "$(printf '%s' "$E" | grep -c carol-fg || true)" "0"

section "Doctor does not condemn a passphrase key it cannot unlock"
mkdir -p "$SB/keys"
ssh-keygen -q -t ed25519 -N secret -C locked -f "$SB/keys/locked" </dev/null
ssh-keygen -q -t ed25519 -N '' -C open -f "$SB/keys/open" </dev/null
printf 'not a key\n' > "$SB/keys/garbage"; printf 'not a key\n' > "$SB/keys/garbage.pub"
FSSH="$SB/fakessh"; mkdir -p "$FSSH"; REAL_SSH="$(command -v ssh)"
cat > "$FSSH/ssh" <<FAKESSH
#!/bin/sh
case "\$1" in -G) exec "$REAL_SSH" "\$@" ;; esac
echo "git@github.com: Permission denied (publickey)." >&2
exit 255
FAKESSH
chmod +x "$FSSH/ssh"
for k in locked open garbage; do
  "$PY" - "$SB/pk-$k.json" "$k" "$SB/keys/$k" "$SB/kdir-$k" <<'JSONPY'
import json, sys
f, name, key, folder = sys.argv[1:]
json.dump({"name": name.capitalize(), "git_name": name, "git_email": name + "@example.test", "ssh_key_path": key, "directories": [folder]}, open(f, "w"))
JSONPY
  mkdir -p "$SB/kdir-$k"; probe profile_apply "$SB" "$SB/pk-$k.json" >/dev/null
done
K=$( PATH="$FSSH:$FGH:$PATH" SSH_AUTH_SOCK= pinvoke doctor_check_keys '{}' )
L=$(printf '%s' "$K" | "$PY" -c 'import sys,json;print("\n".join(f["id"]+"|"+f["title"] for f in json.load(sys.stdin)["value"]))')
ok "a passphrase key github.com refused, not in the agent, is not called unregistered" "$(printf '%s\n' "$L" | grep -c 'key-not-registered|Profile "Locked"' || true)" "0"
ok "  it says it could not verify it, for want of the passphrase" "$L" 'key-locked|Couldn'"'"'t verify profile "Locked"'
ok "  and how to load it" "$K" "ssh-add"
ok "a passphrase-free key github.com refused is still unregistered" "$L" 'key-not-registered|Profile "Open"'
ok "a key ssh-keygen cannot read at all is not called passphrase-protected" "$(printf '%s\n' "$L" | grep -c 'key-locked|Couldn'"'"'t verify profile "Garbage"' || true)" "0"

verify_result
