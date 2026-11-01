#!/bin/bash
# Find repositories, end to end against the real backend in a sandbox.
#
# The search: the clones under the home folder and the places asked for that
# no profile folder holds — never what the Repository page lists already, a
# hidden, media or package folder's, or a submodule's checkout — each with
# whose email it commits with today (the default profile, a remote rule, its
# own config, a profile folder too deep for the page). What GitHub says each
# signed-in account may do with them, against a fake GitHub and a fake gh.
# Adding one to a profile puts it on the Repository page with that profile's
# identity, its identity guard following; Undo takes all of it back. Folders
# added from many threads at once all stay.
set -u
source "$(dirname "$0")/lib.sh"
verify_init findrepos "Find Tester" "find@example.test"
SB="$(realdir "$SB")"
# The profile store lives in the platform data dir, which follows HOME on
# macOS and Linux but not on Windows: there the store would be the real one.
# The walk itself is covered on Windows by the unit tests in find_clones.rs.
if command -v cygpath >/dev/null 2>&1; then
  echo "find-repositories suite: skipped on Windows (the profile store does not follow HOME there)"
  verify_result
  exit $?
fi
unset XDG_DATA_HOME XDG_CONFIG_HOME
# The same folder, spelled with its links resolved (macOS's /var is /private/var):
# the backend's home folder and the paths the suite compares then agree.
H="$(realdir "$HOME")"; export HOME="$H"
GITV="$(git --version | sed -E 's/[^0-9]*([0-9]+\.[0-9]+(\.[0-9]+)?).*/\1/')"

# profile <file> <json>: a profile written into the sandbox store and gitconfig; its id.
profile() { printf '%s' "$2" > "$1"; probe profile_apply "$SB" "$1" | jqf id; }
mkrepo() { git init -q "$1"; if [ -n "${2:-}" ]; then git -C "$1" remote add origin "$2"; fi; }
names() { "$PY" -c "import sys,json; v=json.load(sys.stdin)['value']; print(sorted(r['path'].replace('\\\\','/').rsplit('/',1)[-1] for r in v['repos']))"; }
field() { "$PY" -c "import sys,json; v=json.load(sys.stdin)['value']; r=[r for r in v['repos'] if r['path'].replace('\\\\','/').endswith('/'+sys.argv[1])]; print(r[0]$2 if r else 'missing')" "$1"; }

section "The search finds what the Repository page does not list, and says whose email each commits with"
PID=$(profile "$SB/p1.json" '{"name":"Personal","git_name":"Me","git_email":"me@home.test","directories":[]}')
WORKDIR="$H/work"; mkdir -p "$WORKDIR"
WID=$(profile "$SB/p2.json" "$("$PY" -c 'import json,sys; print(json.dumps({"name":"Work","git_name":"Me At Work","git_email":"me@work.test","directories":[sys.argv[1]],"remote_patterns":["ruleorg/"],"github_login":"work-me"}))' "$WORKDIR")")
ok "(two profiles: Personal the default, Work with a folder and a remote rule)" "$([ -n "$PID" ] && [ -n "$WID" ] && echo made)" "made"
mkrepo "$H/code/mine" "git@github.com:me-home/mine.git"
git -C "$H/code/mine" commit -q --allow-empty -m init
git -C "$H/code/mine" worktree add -q "$H/code/mine-feature" 2>/dev/null
mkrepo "$H/code/tool" "https://github.com/acme/tool.git"
mkrepo "$H/code/ruled" "git@github.com:ruleorg/ruled.git"
mkrepo "$H/code/localid" "git@github.com:me-home/localid.git"
git -C "$H/code/localid" config user.email own@local.test
mkrepo "$H/code/forked" "git@github.com:upstream/forked.git"
git -C "$H/code/forked" config remote.origin.pushurl "git@github.com:me-home/forked.git"
mkrepo "$H/code/plain"
mkrepo "$WORKDIR/listed" "git@github.com:acme/listed.git"
mkrepo "$WORKDIR/a/b/c/d/e/deep" "git@github.com:acme/deep.git"
mkrepo "$H/Library/kept-out" "git@github.com:acme/kept-out.git"
mkrepo "$H/.hidden/kept-out-too" "git@github.com:acme/kept-out-too.git"
mkrepo "$H/code/node_modules/dep" "git@github.com:acme/dep.git"
mkdir -p "$H/code/vendored"; printf 'gitdir: ../mine/.git/modules/vendored\n' > "$H/code/vendored/.git"
mkrepo "$SB/elsewhere/proj" "https://github.com/acme/proj.git"
ARGS=$("$PY" -c 'import json,sys; print(json.dumps({"scanId":"s1","extraRoots":[sys.argv[1], sys.argv[2], "~/code", "relative/x", sys.argv[3]]}))' "$SB/elsewhere" "$WORKDIR/a" "$SB/nope")
OUT=$(pinvoke find_unlisted_repos "$ARGS")
ok "exactly the clones no profile folder lists: under home, in a place outside it, and one too deep inside a profile folder" "$(printf '%s' "$OUT" | names)" "['deep', 'forked', 'localid', 'mine', 'mine-feature', 'plain', 'proj', 'ruled', 'tool']"
ok "  the one the Repository page lists is counted, not offered" "$(printf '%s' "$OUT" | jqf value.already_listed)" "1"
ok "  places that cannot be searched say why" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json; print([(r["place"].rsplit("/",1)[-1] if r["place"].startswith("/") else r["place"], r["why"]) for r in json.load(sys.stdin)["value"]["rejected"]])')" "[('relative/x', 'not a full path'), ('nope', 'not found')]"
ok "  the places, in order: the ones asked for, then home" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json; print([r.replace(sys.argv[1], "~") for r in json.load(sys.stdin)["value"]["roots"]])' "$H")" "/elsewhere', '~/work/a', '~/code', '~']"
ok "a clone outside every folder commits as the default profile" "$(printf '%s' "$OUT" | field mine "['commits_as']")" "{'profile_id': '$PID', 'via': 'default'}"
ok "a clone deeper in a profile folder than the page looks commits as that folder's profile" "$(printf '%s' "$OUT" | field deep "['commits_as']")" "{'profile_id': '$WID', 'via': 'folder'}"
ok "a repository with its own email in .git/config says so" "$(printf '%s' "$OUT" | field localid "['commits_as']")" "{'profile_id': None, 'via': 'local'}"
ok "a worktree is flagged: its identity follows its git folder" "$(printf '%s' "$OUT" | field mine-feature "['gitdir_elsewhere']")" "True"
ok "pushes that go to another repository are checked there" "$(printf '%s' "$OUT" | field forked "['push_owner']+'/'+r[0]['push_name']")" "me-home/forked"
ok "a repository with no remote has no owner" "$(printf '%s' "$OUT" | field plain "['owner'] or 'none'")" "none"
if [ "$(printf '%s\n2.36.0\n' "$GITV" | sort -V | head -1)" = "2.36.0" ]; then
  ok "a clone a remote rule names commits as that rule's profile" "$(printf '%s' "$OUT" | field ruled "['commits_as']")" "{'profile_id': '$WID', 'via': 'rule'}"
else
  echo "  (git $GITV is older than 2.36: remote rules are not available here; skipped)"
fi

section "What each signed-in account may do with each repository, from GitHub"
GHB="$SB/ghbin"; mkdir -p "$GHB"
cat > "$GHB/gh" <<'FAKEGH'
#!/bin/sh
case "$1 $2" in
  "auth status") printf '  ✓ Logged in to github.com account me-home (keyring)\n  - Active account: true\n  ✓ Logged in to github.com account work-me (keyring)\n  - Active account: false\n' >&2; exit 0 ;;
  "auth token") [ "$3" = "--user" ] && { echo "t-$4"; exit 0; } ;;
esac
exit 1
FAKEGH
chmod +x "$GHB/gh"
"$PY" - "$SB/api.log" > "$SB/api-port" <<'PYAPI' &
import http.server, json, sys
log = sys.argv[1]
USERS = {"t-me-home": "Me-Home", "t-work-me": "work-me"}
REPOS = {
    ("me-home/mine", "t-me-home"): (200, {"private": True, "permissions": {"admin": True, "push": True, "pull": True}}),
    ("acme/tool", "t-work-me"): (200, {"private": False, "permissions": {"admin": False, "push": True, "pull": True}}),
    ("bigorg/sso", "t-work-me"): (403, {"message": "Resource protected by organization SAML enforcement."}),
}
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        token = self.headers.get("Authorization", "").split(" ")[-1]
        with open(log, "a") as f:
            f.write(self.path + " " + token + "\n")
        extra = {"X-OAuth-Scopes": "repo, read:org"}
        if self.path == "/user":
            status, body = (200, {"id": 1, "login": USERS[token], "name": None, "email": None, "avatar_url": ""}) if token in USERS else (401, {"message": "Bad credentials"})
        elif self.path.startswith("/repos/"):
            key = self.path[len("/repos/"):].lower()
            status, body = REPOS.get((key, token), (404, {"message": "Not Found"}))
            if status == 403:
                extra["X-GitHub-SSO"] = "required; url=https://github.com/orgs/bigorg/sso"
        else:
            status, body = 404, {}
        data = json.dumps(body).encode()
        self.send_response(status)
        for k, v in extra.items():
            self.send_header(k, v)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(data))); self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
PYAPI
API_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/api-port" ] && break; sleep 0.5; done
API="http://127.0.0.1:$(cat "$SB/api-port")"
M=$( unset GH_TOKEN GITHUB_TOKEN; PATH="$GHB:$PATH" GITSWITCH_GITHUB_API="$API" pinvoke repo_access_matrix '{"scanId":"m1","repos":["me-home/mine","acme/tool","bigorg/sso","ME-HOME/mine"]}' )
cellof() { printf '%s' "$M" | "$PY" -c 'import sys,json; c=json.load(sys.stdin)["value"]["access"].get(sys.argv[1],{}).get(sys.argv[2]); print("none" if c is None else (c["state"], c["push"], c["private"]) if c["state"]!="unknown" else ("unknown", c["why"]))' "$1" "$2"; }
ok "every gh account is asked, under the login GitHub spells" "$(printf '%s' "$M" | "$PY" -c 'import sys,json; print([a["login"] for a in json.load(sys.stdin)["value"]["accounts"]])')" "['Me-Home', 'work-me']"
ok "  each repository once, whatever its letter case" "$(printf '%s' "$M" | "$PY" -c 'import sys,json; print(sorted(json.load(sys.stdin)["value"]["access"]))')" "['acme/tool', 'bigorg/sso', 'me-home/mine']"
ok "an account that can push to a private repository: push, and private" "$(cellof me-home/mine me-home)" "('visible', True, True)"
ok "  an account that cannot see it: invisible, not unknown" "$(cellof me-home/mine work-me)" "('invisible', False, None)"
ok "a public repository one account can push to: not private" "$(cellof acme/tool work-me)" "('visible', True, False)"
ok "an answer single sign-on withholds is unknown, with why" "$(cellof bigorg/sso work-me)" "('unknown', 'single sign-on"
ok "  the tokens went only to the fake GitHub, never into the answer" "$(printf '%s' "$M" | grep -c 't-me-home\|t-work-me' || true)" "0"
{ kill "$API_PID" && wait "$API_PID"; } 2>/dev/null

section "Adding a found clone to a profile, and Undo"
TOOL="$H/code/tool"
OUT=$(pinvoke install_commit_guard "$("$PY" -c 'import json,sys; print(json.dumps({"repoPath":sys.argv[1],"expectedEmail":"me@home.test"}))' "$TOOL")")
guard() { if grep -q "$1" "$TOOL/.git/hooks/pre-commit" && ! grep -q "$2" "$TOOL/.git/hooks/pre-commit"; then echo "expects $1"; else echo "expects something else"; fi; }
ok "(an identity guard in it expects the default profile's email)" "$(guard me@home.test me@work.test)" "expects me@home.test"
ADD=$(pinvoke profile_folders_add "$("$PY" -c 'import json,sys; print(json.dumps({"changes":[{"profile_id":sys.argv[1],"path":sys.argv[2]}]}))' "$WID" "$TOOL")")
ok "the folder is added to Work, as stored, with the guard's email before" "$(printf '%s' "$ADD" | "$PY" -c 'import sys,json; v=json.load(sys.stdin)["value"]; print([(a["path"]==sys.argv[1], a["guard_was"]) for a in v["added"]], v["failed"])' "$TOOL")" "[(True, 'me@home.test')] []"
ok "  it is on the Repository page now, under Work" "$(pinvoke history_list_repos '{}' | "$PY" -c 'import sys,json; print([r["profile_name"] for r in json.load(sys.stdin)["value"] if r["path"]==sys.argv[1]])' "$TOOL")" "['Work']"
ok "  git gives it Work's identity" "$(git -C "$TOOL" config user.email)" "me@work.test"
ok "  and its guard expects Work's email" "$(guard me@work.test me@home.test)" "expects me@work.test"
ok "  the next search counts it as listed" "$(pinvoke find_unlisted_repos '{"scanId":"s3","extraRoots":[]}' | jqf value.already_listed)" "2"
REMOVED=$(printf '%s' "$ADD" | "$PY" -c 'import sys,json; print(json.dumps({"changes": json.load(sys.stdin)["value"]["added"]}))')
OUT=$(pinvoke profile_folders_remove "$REMOVED")
ok "Undo takes it off Work" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json; print([r["profile_id"] for r in json.load(sys.stdin)["value"]])')" "['$WID']"
ok "  it is off the Repository page" "$(pinvoke history_list_repos '{}' | "$PY" -c 'import sys,json; print([r["path"] for r in json.load(sys.stdin)["value"] if r["path"]==sys.argv[1]])' "$TOOL")" "[]"
ok "  git gives it the default identity again" "$(git -C "$TOOL" config user.email)" "me@home.test"
ok "  and its guard expects the default email again" "$(guard me@home.test me@work.test)" "expects me@home.test"
ok "  Work's own folder is untouched" "$(pinvoke get_profiles '{}' | "$PY" -c 'import sys,json; print([p["directories"] for p in json.load(sys.stdin)["value"] if p["id"]==sys.argv[1]][0]==[sys.argv[2]])' "$WID" "$WORKDIR")" "True"

section "Folders added from many threads at once all stay"
DIRS=""; for i in 01 02 03 04 05 06 07 08 09 10 11 12; do mkdir -p "$H/many/r$i"; DIRS="$DIRS${DIRS:+,}$H/many/r$i"; done
OUT=$(probe profile_add_concurrently "$WID" "$DIRS")
ok "every folder is in the store, none lost to another thread's write" "$(printf '%s' "$OUT" | "$PY" -c 'import sys,json; v=json.load(sys.stdin); print(sum(1 for d in v["directories"] if "/many/r" in d.replace("\\\\","/")), v["errors"])')" "12 []"
ok "  and no half-written store is left beside it" "$(ls "$(dirname "$(find "$HOME" -name profiles.json -not -path '*/node_modules/*' | head -1)")" | grep -c '\.tmp$' || true)" "0"

verify_result
