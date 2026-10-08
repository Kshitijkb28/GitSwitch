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

section "Commit Audit lists only your own wrong-identity commits, and fixes them without touching anyone else's"
git init -q --bare "$SB/team.git"
git clone -q "$SB/team.git" "$SB/team-seed" 2>/dev/null
git -C "$SB/team-seed" -c user.name=Mate -c user.email=mate@example.test commit -q --allow-empty -m "mate one"
git -C "$SB/team-seed" push -q origin HEAD:main 2>/dev/null
git -C "$SB/team-seed" checkout -q -b feature
git -C "$SB/team-seed" -c user.name=Mate -c user.email=mate@example.test commit -q --allow-empty -m "mate feature"
git -C "$SB/team-seed" push -q origin feature 2>/dev/null
A="$WORKDIR/team"; git clone -q "$SB/team.git" "$A" 2>/dev/null; git -C "$A" checkout -q main 2>/dev/null
card() { pinvoke audit_commits '{}' | "$PY" -c 'import sys,json;d=[a for a in json.load(sys.stdin)["value"] if a["path"].endswith("/team")];print(sorted(c["subject"] for c in d[0]["mismatched"]) if d else [])'; }
git -C "$A" checkout -q -b solo origin/main --no-track
git -C "$A" -c user.email=wrong@example.test commit -q --allow-empty -m "own wrong"
ok "on a branch with no upstream, only your own commit is listed" "$(card)" "['own wrong']"
git -C "$A" cherry-pick -q origin/feature >/dev/null 2>&1 || git -C "$A" cherry-pick --allow-empty origin/feature >/dev/null 2>&1
ok "  a teammate's commit cherry-picked here is not yours" "$(card)" "['own wrong']"
B="$(git -C "$A" log --format='%s %at' origin/main..HEAD)"
fix() { pinvoke fix_unpushed_commits "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1]}))' "$A")"; }
ok "a branch with no upstream is fixed, not sent to push first" "$(fix)" "Re-authored 1 commit"
ok "  your commit carries the profile now" "$(git -C "$A" log --format='%s|%ae' | grep '^own wrong|')" "own wrong|$EMAIL2"
ok "  the cherry-picked teammate commit keeps its author" "$(git -C "$A" log --format='%s|%ae' | grep '^mate feature|')" "mate feature|mate@example.test"
ok "  author dates are unchanged" "$(git -C "$A" log --format='%s %at' origin/main..HEAD)" "$B"
ok "  the old history is on a backup branch" "$(git -C "$A" branch --list 'gitswitch-backup-*')" "gitswitch-backup-"
ok "  and the repository is clear afterwards: the backup holds the old commits, it is not audited" "$(card)" "[]"
git -C "$A" branch -q notes origin/main
git -C "$A" checkout -q notes; git -C "$A" -c user.email=wrong@example.test commit -q --allow-empty -m "notes wrong"; git -C "$A" checkout -q solo
printf 'x\n' > "$A/notes"
ok "a branch whose name is also a file here is still read" "$(card)" "notes wrong"
rm -f "$A/notes"
# A contributor's commits fetched straight into a branch — a pull request
# checked out — are author and committer at once, but were not made here.
git -C "$SB/team-seed" checkout -q -b contrib main
git -C "$SB/team-seed" -c user.name=Contributor -c user.email=contrib@example.test commit -q --allow-empty -m "contributor pr"
git -C "$A" fetch -q "$SB/team-seed" contrib:pr-1 2>/dev/null
ok "a contributor's commits fetched into a branch of their own are not listed as yours" "$(card | "$PY" -c 'import sys;print("contributor pr" in sys.stdin.read())')" "False"
# A single-branch clone: a pushed branch gets no remote-tracking branch here.
N="$WORKDIR/narrow"; git clone -q --single-branch --branch main "$SB/team.git" "$N" 2>/dev/null
git -C "$N" checkout -q -b topic
git -C "$N" -c user.email=wrong@example.test commit -q --allow-empty -m "narrow wrong"
git -C "$N" push -q -u origin topic 2>/dev/null
H="$(git -C "$N" rev-parse HEAD)"
ok "(pushed, though no remote-tracking branch says so here)" "$(git -C "$SB/team.git" rev-parse topic):$(git -C "$N" rev-parse --verify -q origin/topic || echo none)" "$H:none"
ok "a branch whose upstream has no remote-tracking branch here is not rewritten" "$(pinvoke fix_unpushed_commits "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1]}))' "$N")")" "no remote-tracking branch here"
ok "  and nothing moved" "$(git -C "$N" rev-parse HEAD)" "$H"
# A signed wrong commit with log.showSignature on: the rewrite still reads
# the author alone, not the signature's report around it.
S="$WORKDIR/signed"; git clone -q "$SB/team.git" "$S" 2>/dev/null
ssh-keygen -q -t ed25519 -N "" -C sig -f "$SB/sigkey" >/dev/null
printf 'wrong@example.test %s\n' "$(cat "$SB/sigkey.pub")" > "$SB/allowed_signers"
git -C "$S" checkout -q -b sig origin/main --no-track
git -C "$S" -c user.email=wrong@example.test -c gpg.format=ssh -c user.signingkey="$SB/sigkey.pub" commit -q -S --allow-empty -m "signed wrong"
git -C "$S" config gpg.ssh.allowedSignersFile "$SB/allowed_signers"; git -C "$S" config log.showSignature true
ok "(git log prints the signature's report beside the author here)" "$(git -C "$S" log -1 --format=%ae | head -1)" "Good"
pinvoke fix_unpushed_commits "$("$PY" -c 'import json,sys;print(json.dumps({"repoPath":sys.argv[1]}))' "$S")" >/dev/null
ok "a signed wrong commit is re-authored even with log.showSignature on" "$(git -C "$S" log -1 --no-show-signature --format=%ae)" "$EMAIL2"
git -C "$A" checkout -q main
git -C "$A" merge -q --no-ff origin/feature -m "merge feature" >/dev/null 2>&1
ok "a merged-in teammate branch is not listed as yours" "$(card | "$PY" -c 'import sys;print("mate feature" in sys.stdin.read())')" "False"
git -C "$A" checkout -q -b mixed origin/main --no-track
git -C "$A" -c user.email=wrong@example.test commit -q --allow-empty -m "mixed one"
git -C "$A" merge -q --no-ff origin/feature -m "merge in mixed" >/dev/null 2>&1
git -C "$A" -c user.email=wrong@example.test commit -q --allow-empty -m "mixed two"
H="$(git -C "$A" rev-parse HEAD)"
ok "a merge among the commits to rewrite is refused" "$(fix)" "merge"
ok "  and nothing moved" "$(git -C "$A" rev-parse HEAD)" "$H"
git -C "$A" checkout -q -b ovr origin/main --no-track
git -C "$A" -c user.email=wrong@example.test commit -q --allow-empty -m "ovr wrong"
git -C "$A" config user.email someone-else@example.test
H="$(git -C "$A" rev-parse HEAD)"
ok "a repository-local user.email is refused, not 'fixed' with the same wrong address" "$(fix)" "user.email"
ok "  and nothing moved" "$(git -C "$A" rev-parse HEAD)" "$H"
git -C "$A" config --unset user.email

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
    def do_POST(self):
        auth = self.headers.get("Authorization", "")
        self.rfile.read(int(self.headers.get("Content-Length", "0") or 0))
        with open(log, "a") as f:
            f.write("POST " + self.path + " " + auth.split(" ")[-1] + "\n")
        if self.path == "/user/keys" and auth.endswith("t-scoped"):
            body = json.dumps({"id": 1, "key": "ssh-ed25519 AAAA", "title": "kept"}).encode()
            self.send_response(201)
        else:
            body = json.dumps({"message": "Bad credentials"}).encode()
            self.send_response(401)
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

# A "Sign in with GitHub" is kept: its token in the credential store (in the
# suites a file store in the sandbox — never the real keychain), its login in
# GitSwitch's list, under the login GitHub names for the token.
OUT=$(GITSWITCH_GITHUB_API="$EAPI" pinvoke github_signin_keep '{"token":"t-scoped"}')
ok "a sign-in is kept under the login GitHub names for its token" "$(printf '%s' "$OUT" | jqf value.login):$(cat "$GITSWITCH_TOKEN_DIR/github_octo-home" 2>/dev/null)" "octo-home:t-scoped"
ok "  listed as kept, with no token in the list" "$(pinvoke github_signins '{}' | jqf value):$(grep -c t-scoped "$GITSWITCH_TOKEN_DIR/github-signins.json" || true)" '[{"id": 42, "login": "octo-home"}]:0'
ok "  and offered on the Repositories page as an account" "$(pinvoke repo_accounts '{}' | "$PY" -c 'import sys,json;print([a["key"] for a in json.load(sys.stdin)["value"] if a["key"].startswith("signin:")])')" "['signin:octo-home']"
if command -v ssh-keygen >/dev/null 2>&1; then
  ssh-keygen -q -t ed25519 -N '' -C kept -f "$SB/kept_key" </dev/null
  OUT=$(GITSWITCH_GITHUB_API="$EAPI" pinvoke gh_register_ssh_key "{\"account\":\"signin:octo-home\",\"keyPath\":\"$SB/kept_key\"}")
  ok "a key is registered with the kept sign-in's own token, no gh involved" "$(printf '%s' "$OUT" | jqf value):$(grep -c 'POST /user/keys t-scoped' "$SB/emails-api.log")" "Registered on octo-home:1"
  GITSWITCH_GITHUB_API="$EAPI" pinvoke github_signin_keep '{"token":"t-plain"}' >/dev/null
  OUT=$(GITSWITCH_GITHUB_API="$EAPI" pinvoke gh_register_ssh_key "{\"account\":\"signin:octo-home\",\"keyPath\":\"$SB/kept_key\"}")
  ok "  a kept sign-in GitHub no longer accepts says to sign in again on the GitHub page, not with gh" "$(printf '%s' "$OUT" | jqf error)" "sign in again on the GitHub page"
fi
pinvoke github_signin_forget '{"login":"Octo-Home"}' >/dev/null
ok "Forget removes the token and the login, whatever the letter case" "$([ -e "$GITSWITCH_TOKEN_DIR/github_octo-home" ] && echo kept || echo gone):$(pinvoke github_signins '{}' | jqf value)" "gone:[]"
OUT=$(pinvoke repo_accounts '{}' | "$PY" -c 'import sys,json;print([a["key"] for a in json.load(sys.stdin)["value"] if a["key"].startswith("signin:")])')
ok "  and it is no longer offered" "$OUT" "[]"
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

section "A default profile that signs, signs everywhere no other profile applies"
if command -v ssh-keygen >/dev/null 2>&1 && ssh-keygen -Y sign -? 2>&1 | grep -q -- '-n namespace\|namespace'; then
  ssh-keygen -q -t ed25519 -N '' -C default -f "$SB/home/.ssh/sign_default" </dev/null
  ssh-keygen -q -t ed25519 -N '' -C signer -f "$SB/home/.ssh/sign_signer" </dev/null
  DID=$(pinvoke get_profiles '{}' | "$PY" -c 'import sys,json;print([p["id"] for p in json.load(sys.stdin)["value"] if p["is_default"]][0])')
  printf '{"id":"%s","ssh_key_path":"%s","signing_enabled":true}' "$DID" "$SB/home/.ssh/sign_default" > "$SB/psigndef.json"
  probe profile_update "$SB" "$SB/psigndef.json" >/dev/null
  mkdir -p "$SB/SignerDir"
  "$PY" - "$SB/psigner.json" "$SB/home/.ssh/sign_signer" "$SB/SignerDir" <<'JSONPY'
import json, sys
f, key, folder = sys.argv[1:]
json.dump({"name": "Signer", "git_name": "Signer", "git_email": "signer@example.test", "ssh_key_path": key, "directories": [folder], "signing_enabled": True}, open(f, "w"))
JSONPY
  probe profile_apply "$SB" "$SB/psigner.json" >/dev/null
  git init -q "$SB/outside-signed"; git -C "$SB/outside-signed" commit -q --allow-empty -m x
  git init -q "$WORKDIR/nosign"; git -C "$WORKDIR/nosign" commit -q --allow-empty -m x
  git init -q "$SB/SignerDir/repo"; git -C "$SB/SignerDir/repo" commit -q --allow-empty -m x
  ok "outside every folder, the default's commit is signed and verifies" "$(git -C "$SB/outside-signed" log -1 --format=%G?)" "G"
  ok "  as the default's address" "$(git -C "$SB/outside-signed" log -1 --format=%GS)" "$EMAIL1"
  ok "a folder profile that does not sign is not signed with the default's key" "$(git -C "$WORKDIR/nosign" log -1 --format=%G?)" "N"
  ok "a signing folder profile signs with its own key" "$(git -C "$SB/SignerDir/repo" log -1 --format='%G? %GS')" "G signer@example.test"
else
  echo "  (this ssh-keygen cannot sign: skipped)"
fi

section "Doctor repairs a folder that no longer exists, and refuses what it would not offer"
"$PY" - "$SB/pgone.json" "$SB/never-made" <<'JSONPY'
import json, sys
json.dump({"name": "Gone", "git_name": "Gone", "git_email": "gone@example.test", "directories": [sys.argv[2]]}, open(sys.argv[1], "w"))
JSONPY
probe profile_apply "$SB" "$SB/pgone.json" >/dev/null
P=$(pinvoke doctor_check_profiles '{}')
ok "the missing folder offers a repair" "$P" "drop-folder:"
FIX=$(printf '%s' "$P" | "$PY" -c 'import sys,json;print([f["fix"] for f in json.load(sys.stdin)["value"] if f["id"]=="folder-missing" and "never-made" in f["detail"]][0])')
R=$(pinvoke doctor_apply_fix "$("$PY" -c 'import json,sys;print(json.dumps({"fix":sys.argv[1]}))' "$FIX")")
ok "the repair says what it removed" "$R" "Removed"
ok "  the profile no longer lists it" "$(pinvoke get_profiles '{}' | grep -c never-made || true)" "0"
ok "  nor does git's config" "$(grep -c never-made "$HOME/.gitconfig" || true)" "0"
ok "  so the finding is gone" "$(pinvoke doctor_check_profiles '{}' | grep -c never-made || true)" "0"
R=$(pinvoke doctor_apply_fix "$("$PY" -c 'import json,sys;print(json.dumps({"fix":"drop-folder:"+sys.argv[1]+":"+sys.argv[2]}))' "$WID" "$WORKDIR")")
ok "a folder that exists and no other profile lists is not removed" "$R" "nothing to repair"
ok "  and stays" "$(pinvoke get_profiles '{}' | "$PY" -c 'import sys,json;print(any(sys.argv[1] in p["directories"] for p in json.load(sys.stdin)["value"]))' "$WORKDIR")" "True"
# A folder two profiles claim (a hand-edited store), with a guard expecting
# the address git gives it now: dropping it from that profile refreshes the
# guard to the profile that keeps it, so commits there still go through.
mkdir -p "$SB/twice"; git init -q "$SB/twice/api"
for n in TwiceA TwiceB; do
  "$PY" - "$SB/p$n.json" "$n" "$SB/twice-$n" <<'JSONPY'
import json, sys
f, name, folder = sys.argv[1:]
json.dump({"name": name, "git_name": name, "git_email": name.lower() + "@example.test", "directories": [folder]}, open(f, "w"))
JSONPY
  probe profile_apply "$SB" "$SB/p$n.json" >/dev/null
done
"$PY" - "$STORE" "$SB/twice" <<'STOREPY'
import json, sys
f, folder = sys.argv[1:]
d = json.load(open(f))
for p in d["profiles"]:
    if p["name"] in ("TwiceA", "TwiceB"):
        p["directories"] = [folder]
json.dump(d, open(f, "w"))
STOREPY
pinvoke apply_git_config '{}' >/dev/null
BEFORE_EMAIL="$(git -C "$SB/twice/api" config user.email)"
probe guard_install "$SB/twice/api" "$BEFORE_EMAIL" >/dev/null
P=$(pinvoke doctor_check_profiles '{}')
FIX=$(printf '%s' "$P" | "$PY" -c 'import sys,json;print([f["fix"] for f in json.load(sys.stdin)["value"] if f["id"]=="folder-conflict" and (f.get("fix") or "").startswith("drop-folder:") and sys.argv[1] in f["detail"]][0])' "$SB/twice")
pinvoke doctor_apply_fix "$("$PY" -c 'import json,sys;print(json.dumps({"fix":sys.argv[1]}))' "$FIX")" >/dev/null
NOW_EMAIL="$(git -C "$SB/twice/api" config user.email)"
ok "(the folder now belongs to the other profile)" "$BEFORE_EMAIL -> $NOW_EMAIL" "twiceb@example.test -> twicea@example.test"
ok "dropping a folder two profiles claimed refreshes its guard to the profile that keeps it" "$(probe guard_email "$SB/twice/api")" "$NOW_EMAIL"
ok "  so a commit there still goes through" "$(git -C "$SB/twice/api" commit -q --allow-empty -m after 2>&1; git -C "$SB/twice/api" log -1 --format=%ae 2>/dev/null)" "$NOW_EMAIL"

section "Doctor asks GitHub whether the account a repository pushes as can see it"
AB="$SB/accbin"; mkdir -p "$AB" "$SB/acc/home" "$SB/acc/work" "$SB/keys"
printf 'not a key\n' > "$SB/keys/octo-home"; printf 'not a key\n' > "$SB/keys/octo-work"
ACC_SSH="$(command -v ssh)"
cat > "$AB/ssh" <<FAKESSH
#!/bin/sh
case "\$1" in -G|-F) exec "$ACC_SSH" "\$@" ;; esac
key=""; prev=""
for a in "\$@"; do [ "\$prev" = "-i" ] && key="\$a"; prev="\$a"; done
echo "Hi \$(basename "\$key")! You've successfully authenticated, but GitHub does not provide shell access." >&2
exit 1
FAKESSH
cat > "$AB/gh" <<'FAKEGH'
#!/bin/sh
case "$1 $2" in
  "auth status") printf '  ✓ Logged in to github.com account octo-home (keyring)\n  ✓ Logged in to github.com account octo-work (keyring)\n' >&2; exit 0;;
  "auth token") shift 2; while [ $# -gt 0 ]; do [ "$1" = "--user" ] && echo "tok-$2"; shift; done; exit 0;;
esac
exit 1
FAKEGH
chmod +x "$AB/ssh" "$AB/gh"
printf 'Host *\n  IdentityFile %s\n' "$SB/keys/none" > "$SB/acc-sshcfg"
for who in home work; do
  "$PY" - "$SB/pacc-$who.json" "$who" "$SB/keys/octo-$who" "$SB/acc/$who" <<'JSONPY'
import json, sys
f, who, key, folder = sys.argv[1:]
p = {"name": "Acc" + who.capitalize(), "git_name": "octo-" + who, "git_email": who + "@octo.test", "ssh_key_path": key, "directories": [folder]}
if who == "work": p["github_login"] = "octo-work"
json.dump(p, open(f, "w"))
JSONPY
  probe profile_apply "$SB" "$SB/pacc-$who.json" >/dev/null
done
for r in thing sso open noscope; do git init -q "$SB/acc/home/$r"; git -C "$SB/acc/home/$r" remote add origin "git@github.com:acme/$r.git"; done
"$PY" - "$SB/acc-api.log" > "$SB/acc-api-port" <<'PYAPI' &
import http.server, sys, json
log = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        auth = self.headers.get("Authorization", "")
        with open(log, "a") as f:
            f.write("%s %s\n" % (self.path, auth.split()[-1] if auth else "-"))
        status, headers, body = 404, {}, {"message": "Not Found"}
        if self.path == "/repos/acme/thing":
            if auth.endswith("tok-octo-work"):
                status, body = 200, {"permissions": {"push": True, "pull": True}}
            else:
                headers["X-OAuth-Scopes"] = "repo, read:org"
        elif self.path == "/repos/acme/sso":
            status, headers = 403, {"X-GitHub-SSO": "required; url=https://github.com/orgs/acme/sso"}
        elif self.path == "/repos/acme/open":
            status, body = 200, {"permissions": {"push": True, "pull": True}}
        elif self.path == "/repos/acme/noscope":
            headers["X-OAuth-Scopes"] = "read:user"
        elif self.path == "/repos/acme/viahttps":
            if auth.endswith("tok-octo-work"):
                status, body = 200, {"permissions": {"push": True, "pull": True}}
            else:
                headers["X-OAuth-Scopes"] = "repo"
        data = json.dumps(body).encode()
        self.send_response(status)
        for k, v in headers.items():
            self.send_header(k, v)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(data))); self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
PYAPI
ACC_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/acc-api-port" ] && break; sleep 0.5; done
ACC_API="http://127.0.0.1:$(cat "$SB/acc-api-port")"
accx() { ( unset SSH_AUTH_SOCK GH_TOKEN GITHUB_TOKEN; PATH="$AB:$PATH" GITSWITCH_SSH_CONFIG="$SB/acc-sshcfg" GITSWITCH_GITHUB_API="$ACC_API" pinvoke "$@" ); }
D=$(accx doctor_check_repo_access '{}')
ok "an account that cannot see the repository is named" "$D" "@octo-home cannot see acme/thing"
ok "  with a move to the profile whose account can push there" "$D" "assign:"
ok "a repository behind single sign-on is not called unreachable" "$(printf '%s' "$D" | grep -c 'cannot see acme/sso' || true)" "0"
ok "  nor one a token without the repo scope cannot see" "$(printf '%s' "$D" | grep -c 'cannot see acme/noscope' || true)" "0"
ok "  nor one the account can see" "$(printf '%s' "$D" | grep -c 'cannot see acme/open' || true)" "0"
FIX=$(printf '%s' "$D" | "$PY" -c 'import sys,json;print([f["fix"] for f in json.load(sys.stdin)["value"] if f["id"].startswith("remote-not-reachable:") and f["fix"]][0])')
R=$(accx doctor_apply_fix "$("$PY" -c 'import json,sys;print(json.dumps({"fix":sys.argv[1]}))' "$FIX")")
ok "moving it applies" "$R" 'now belongs to \"AccWork\"'
ok "  and git gives it that profile" "$(git -C "$SB/acc/home/thing" config user.email)" "work@octo.test"
# Over HTTPS the credential helper decides the account, not the folder's
# profile: moving the repository would change nothing, so Convert to SSH is
# what is offered — never a move.
git init -q "$SB/acc/home/viahttps"; git -C "$SB/acc/home/viahttps" remote add origin "https://github.com/acme/viahttps.git"
"$PY" - "$STORE" <<'STOREPY'
import json, sys
d = json.load(open(sys.argv[1]))
for p in d["profiles"]:
    if p["name"] == "AccHome":
        p["github_login"] = "octo-home"
json.dump(d, open(sys.argv[1], "w"))
STOREPY
D=$(accx doctor_check_repo_access '{}')
ok "an HTTPS repository its account cannot see is offered Convert to SSH, not a move to another profile" "$(printf '%s' "$D" | "$PY" -c 'import sys,json;print([f["fix"] for f in json.load(sys.stdin)["value"] if f["id"].startswith("remote-not-reachable:") and f["id"].endswith("/viahttps")])')" "['convert-ssh:"
{ kill "$ACC_PID" && wait "$ACC_PID"; } 2>/dev/null

section "Convert to SSH: GitHub's own hosts only, every URL git pushes with, submodules and deeper folders too"
CV="$SB/convert"; mkdir -p "$CV/deep/1/2/3"
"$PY" - "$SB/pcv.json" "$CV" <<'JSONPY'
import json, sys
json.dump({"name": "Convert", "git_name": "C", "git_email": "c@example.test", "directories": [sys.argv[2]]}, open(sys.argv[1], "w"))
JSONPY
probe profile_apply "$SB" "$SB/pcv.json" >/dev/null
git init -q "$CV/a"; git -C "$CV/a" remote add origin "https://user:ghp_0123456789abcdefghijklmnopqrstuvwxyzAB@GitHub.com:443/acme/a.git/"
git -C "$CV/a" config remote.origin.pushurl "https://x-access-token:secret@github.com/acme/a"
git -C "$CV/a" config gitswitch.savedpushurl.origin "https://github.com/acme/a.git"
git init -q "$CV/pushonly"; git -C "$CV/pushonly" remote add origin "git@github.com:acme/p.git"; git -C "$CV/pushonly" config remote.origin.pushurl "https://x-access-token:secret@github.com/acme/p.git"
git init -q "$CV/gist"; git -C "$CV/gist" remote add origin "https://gist.github.com/0123abcd.git"
git init -q "$CV/mirror"; git -C "$CV/mirror" remote add origin "https://github.com.mirror.example/acme/m.git"
git init -q "$CV/lab"; git -C "$CV/lab" remote add origin "https://gitlab.com/acme/l.git"
git init -q "$CV/deep/1/2/3/five"; git -C "$CV/deep/1/2/3/five" remote add origin "https://github.com/acme/five.git"
git init -q --bare "$CV/lib.git"; git clone -q "$CV/lib.git" "$CV/lib-seed" 2>/dev/null; printf 'x\n' > "$CV/lib-seed/x"; git -C "$CV/lib-seed" add -A >/dev/null; git -C "$CV/lib-seed" commit -qm lib; git -C "$CV/lib-seed" push -q origin main 2>/dev/null
mv "$CV/lib-seed" "$SB/lib-seed-outside"
git init -q "$CV/super"; git -C "$CV/super" remote add origin "https://github.com/acme/super.git"
( cd "$CV/super" && git submodule add -q "$CV/lib.git" lib 2>/dev/null ); git -C "$CV/super" commit -qm "add lib"
git -C "$CV/super/lib" remote set-url origin "https://github.com/acme/lib.git"; git -C "$CV/super" config submodule.lib.url "https://github.com/acme/lib.git"
D=$(pinvoke doctor_check_repos '{}')
fix_of() { printf '%s' "$D" | "$PY" -c "import sys,json;print([f.get('fix') for f in json.load(sys.stdin)['value'] if f['id'].startswith('$1') and f['detail'].startswith('$2')])"; }
ok "Doctor sees a token in a push URL, not only in the fetch URL" "$(fix_of remote-push-embedded-token "$CV/pushonly")" "['convert-ssh:$CV/pushonly']"
ok "  and offers Convert to SSH for a gist, which has an SSH address of its own" "$(fix_of remote-https "$CV/gist")" "['convert-ssh:$CV/gist']"
ok "  but not for a host that merely contains github.com" "$(fix_of remote-https "$CV/mirror")" "[None]"
ok "  nor for another host" "$(fix_of remote-https "$CV/lab")" "[None]"
OUT=$(pinvoke convert_repos_to_ssh "{\"directory\":\"$CV\"}")
ok "a GitHub remote in another letter case, with a port and a token, becomes its SSH form" "$(git -C "$CV/a" config remote.origin.url)" "git@github.com:acme/a.git"
ok "  its push URL too" "$(git -C "$CV/a" config remote.origin.pushurl)" "git@github.com:acme/a.git"
ok "  and the push URL a lock keeps aside for unlocking" "$(git -C "$CV/a" config gitswitch.savedpushurl.origin)" "git@github.com:acme/a.git"
ok "  no token is left in its config" "$(grep -c 'ghp_\|secret' "$CV/a/.git/config" || true)" "0"
ok "  and the result says what moved" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json;print([c["note"] for c in json.load(sys.stdin)["value"] if c["repo"].endswith("/a")])')" "Converted to SSH: fetch URL, push URL, saved push URL; token removed"
ok "a push URL is converted where the fetch URL already was SSH" "$(git -C "$CV/pushonly" config remote.origin.pushurl)" "git@github.com:acme/p.git"
ok "a gist becomes the gist's own SSH address" "$(git -C "$CV/gist" config remote.origin.url)" "git@gist.github.com:0123abcd.git"
ok "a host that merely contains github.com is left alone" "$(git -C "$CV/mirror" config remote.origin.url)" "https://github.com.mirror.example/acme/m.git"
ok "  as is another host" "$(git -C "$CV/lab" config remote.origin.url)" "https://gitlab.com/acme/l.git"
ok "a repository five folders down is reached, as Doctor's scan reaches it" "$(git -C "$CV/deep/1/2/3/five" config remote.origin.url)" "git@github.com:acme/five.git"
ok "a populated submodule is converted too" "$(git -C "$CV/super/lib" config remote.origin.url)" "git@github.com:acme/lib.git"
ok "  and the superproject's own record of its address" "$(git -C "$CV/super" config submodule.lib.url)" "git@github.com:acme/lib.git"
ok "  never the committed .gitmodules" "$(git -C "$CV/super" config -f .gitmodules submodule.lib.url):$(git -C "$CV/super" status --porcelain | wc -l | tr -d ' ')" "$CV/lib.git:0"
# A hand-written .gitmodules naming the repository itself and a folder
# outside it: the conversion finishes, and touches nothing outside.
LP="$SB/loopy"; git init -q "$LP"; git -C "$LP" remote add origin "https://github.com/acme/loopy.git"
git init -q "$SB/outside"; git -C "$SB/outside" remote add origin "https://github.com/acme/outside.git"
printf '[submodule "self"]\n\tpath = .\n\turl = https://github.com/acme/loopy.git\n[submodule "away"]\n\tpath = %s\n\turl = https://github.com/acme/outside.git\n' "$SB/outside" > "$LP/.gitmodules"
( pinvoke convert_repos_to_ssh "{\"directory\":\"$LP\"}" > "$SB/loopy.out" ) & LPID=$!
for i in $(seq 1 60); do kill -0 "$LPID" 2>/dev/null || break; sleep 0.5; done
if kill -0 "$LPID" 2>/dev/null; then pkill -P "$LPID" 2>/dev/null; kill "$LPID" 2>/dev/null; LOOPED=hung; else LOOPED=finished; fi
ok "a .gitmodules naming the repository itself does not send the conversion round in circles" "$LOOPED:$(git -C "$LP" config remote.origin.url)" "finished:git@github.com:acme/loopy.git"
ok "  and a path outside the folder is left alone" "$(git -C "$SB/outside" config remote.origin.url)" "https://github.com/acme/outside.git"

section "HTTPS clones push over SSH with their profile's key"
mkdir -p "$SB/keys"; ssh-keygen -q -t ed25519 -N '' -C overssh -f "$SB/keys/overssh" </dev/null
for spec in "ViaSsh:viaSSH:on" "ViaHttps:viaHTTPS:" "ViaBlocked:viaBlocked:blocked"; do
  IFS=: read -r name folder flag <<<"$spec"
  mkdir -p "$SB/$folder"
  "$PY" - "$SB/p$name.json" "$name" "$SB/keys/overssh" "$SB/$folder" "$flag" <<'JSONPY'
import json, sys
f, name, key, folder, flag = sys.argv[1:]
p = {"name": name, "git_name": name, "git_email": name.lower() + "@example.test", "ssh_key_path": key, "directories": [folder]}
if flag == "on": p["push_over_ssh"] = True
if flag == "blocked": p["allow_push"] = False; p["push_over_ssh"] = True
json.dump(p, open(f, "w"))
JSONPY
  probe profile_apply "$SB" "$SB/p$name.json" >/dev/null
  git init -q "$SB/$folder/app"; git -C "$SB/$folder/app" remote add origin "https://github.com/acme/app.git"
done
ok "an HTTPS clone in a profile that turned it on pushes over SSH, with its key" "$(git -C "$SB/viaSSH/app" remote get-url --push origin) | $(git -C "$SB/viaSSH/app" config core.sshCommand | grep -c overssh)" "git@github.com:acme/app.git | 1"
ok "  and still fetches over HTTPS" "$(git -C "$SB/viaSSH/app" remote get-url origin)" "https://github.com/acme/app.git"
ok "a profile that did not turn it on pushes its HTTPS clones over HTTPS, as before" "$(git -C "$SB/viaHTTPS/app" remote get-url --push origin)" "https://github.com/acme/app.git"
ok "a profile whose pushes are blocked blocks them — never pushes over SSH instead" "$(git -C "$SB/viaBlocked/app" remote get-url --push origin)" "gitswitch-push-blocked://acme/app.git"
probe push_mode "$SB/viaSSH/app" "guardrail" >/dev/null
ok "a repository's own block still wins over its profile's SSH push" "$(git -C "$SB/viaSSH/app" remote get-url --push origin)" "gitswitch-push-blocked://"
probe push_mode "$SB/viaSSH/app" "allowed" >/dev/null
OUT=$(pinvoke get_profiles '{}' | "$PY" -c 'import sys,json;print(sorted((p["name"], p.get("push_over_ssh")) for p in json.load(sys.stdin)["value"] if p["name"].startswith("Via")))')
ok "  (the setting is stored as given: unset means off)" "$OUT" "[('ViaBlocked', True), ('ViaHttps', None), ('ViaSsh', True)]"

verify_result
