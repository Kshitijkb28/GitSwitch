#!/bin/bash
# Profile folders that hold no repository, end to end against the real
# backend in a sandbox.
#
# The Repository page lists such a folder (a project git doesn't know yet,
# or a folder inside a repository) instead of letting it vanish. It leaves
# out a folder that holds repositories (they are listed themselves) and one
# that is gone. "Make it a repository" runs git init there:
#   - the first branch is main, or the person's init.defaultBranch;
#   - the profile's identity applies at once, through its gitdir rule;
#   - nothing is written where git init would be wrong (already a
#     repository, inside one, holding some, not there).
set -u
source "$(dirname "$0")/lib.sh"
verify_init folders "Folder Tester" "folders@example.test"     # HOME redirected FIRST
SB="$(realdir "$SB")"

v() { "$PY" -c "import sys,json; v=json.load(sys.stdin)['value']; print($1)"; }
err() { "$PY" -c "import sys,json; print(json.load(sys.stdin).get('error') or 'NO ERROR')"; }
jstr() { "$PY" -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }
# The list as "name:kind[:inside <name>]", sorted: what the picker offers.
entries() {
  pinvoke history_list_repos '{}' | "$PY" -c '
import sys, json
last = lambda p: p.replace("\\", "/").rstrip("/").rsplit("/", 1)[-1]
out = []
for r in json.load(sys.stdin)["value"]:
    inside = r.get("inside")
    out.append(last(r["path"]) + ":" + r.get("kind", "repo") + (":inside " + last(inside) if inside else ""))
print(" ".join(sorted(out)))'
}

P="$SB/places"
mkdir -p "$P/project/app/src" "$P/holder" "$P/repo/sub" "$P/other"
printf 'a project\n' > "$P/project/README.md"
for r in holder/one holder/two repo; do git init -q "$P/$r"; done
# The default profile first, so Work's email inside its folders can only come from Work's own rule.
printf '{"name":"Personal","git_name":"Me","git_email":"me@home.test","directories":[]}' > "$SB/p0.json"
probe profile_apply "$SB" "$SB/p0.json" >/dev/null
"$PY" -c 'import json,sys; p=sys.argv[1]; print(json.dumps({"name":"Work","git_name":"Work Person","git_email":"work@example.test","directories":[p+"/project",p+"/holder",p+"/repo/sub",p+"/gone"]}))' "$P" > "$SB/p.json"
PID=$(probe profile_apply "$SB" "$SB/p.json" | jqf id)
ok "(a Work profile over a project, a folder of two repositories, a folder inside a repository and one that is gone)" "$([ -n "$PID" ] && echo made)" "made"

section "Every profile folder shows on the Repository page"
ok "the project is listed as a folder; the two repositories, not the folder holding them; the folder inside a repository says so; the gone one is left out" \
  "$(entries)" "one:repo project:folder sub:folder:inside repo two:repo"
ok "  the project is Work's" "$(pinvoke history_list_repos '{}' | v "[r['profile_name'] for r in v if r['path'].replace(chr(92),'/').endswith('/project')]")" "['Work']"

section "What each folder is to git"
fs() { pinvoke folder_state "{\"path\":$(jstr "$1")}" | v "v['kind'] + (':' + str(v['count']) if 'count' in v else '') + (':inside' if 'top' in v else '')"; }
ok "a project git doesn't know" "$(fs "$P/project")" "no-repo"
ok "a folder of repositories" "$(fs "$P/holder")" "contains-repos:2"
ok "a repository" "$(fs "$P/repo")" "repo"
ok "a folder inside one" "$(fs "$P/repo/sub")" "inside-repo:inside"
ok "a folder that is gone" "$(fs "$P/gone")" "missing"

section "Make it a repository"
OUT=$(pinvoke repo_init "{\"path\":$(jstr "$P/project")}")
ok "it is one now, on main" "$(printf '%s' "$OUT" | v "v['branch']") $([ -d "$P/project/.git" ] && echo made)" "main made"
ok "  its files are as they were" "$(cat "$P/project/README.md")" "a project"
ok "  Work's identity applies at once, through its gitdir rule" "$(git -C "$P/project" config user.email)" "work@example.test"
ok "  the list now calls it a repository" "$(entries)" "one:repo project:repo sub:folder:inside repo two:repo"
ok "  with no commits yet" "$(pinvoke changes_repo_status "{\"repoPath\":$(jstr "$P/project")}" | v "v['unborn']")" "True"

git config --global init.defaultBranch trunk
OUT=$(pinvoke repo_init "{\"path\":$(jstr "$P/other")}")
ok "a git set to another first branch keeps it" "$(printf '%s' "$OUT" | v "v['branch']")" "trunk"
git config --global --unset init.defaultBranch

section "Nothing is written where git init would be wrong"
ok "already a repository: refused" "$(pinvoke repo_init "{\"path\":$(jstr "$P/project")}" | err)" "already a git repository"
ok "inside a repository: refused, naming it" "$(pinvoke repo_init "{\"path\":$(jstr "$P/repo/sub")}" | err)" "inside the repository"
ok "  and nothing was made there" "$([ -e "$P/repo/sub/.git" ] && echo MADE || echo none)" "none"
ok "holding repositories: refused" "$(pinvoke repo_init "{\"path\":$(jstr "$P/holder")}" | err)" "already holds 2 repositories"
ok "  and nothing was made around them" "$([ -e "$P/holder/.git" ] && echo MADE || echo none)" "none"
ok "a folder that is gone: refused" "$(pinvoke repo_init "{\"path\":$(jstr "$P/gone")}" | err)" "not on this computer"
ok "  and it was not created" "$([ -e "$P/gone" ] && echo CREATED || echo none)" "none"

section "Publish a repository made here: no remote, then its address added"
PUB="$SB/publish"; mkdir -p "$PUB"
git init -q -b main "$PUB" && printf 'x\n' > "$PUB/a.txt" && git -C "$PUB" add -A && git -C "$PUB" commit -qm "first" >/dev/null
BARE="$SB/publish-remote.git"; git init -q --bare "$BARE"
OUT=$(pinvoke changes_push "{\"repoPath\":$(jstr "$PUB"),\"setUpstream\":true}")
ok "Publish with no remote is refused as no-remote" "$(printf '%s' "$OUT" | v "(v.get('refusal') or {}).get('code')")" "no-remote"
ok "  saying it is on no server yet, not sending you to the terminal" "$(printf '%s' "$OUT" | v "'terminal' not in v['refusal']['message'] and 'no remote to push to' in v['refusal']['message']")" "True"
ok "a transport-helper address is refused" "$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":$(jstr "ext::sh -c touch% $SB/pwned")}" | err)" "transport helpers"
ok "  nothing ran and no remote was added" "ran=$([ -e "$SB/pwned" ] && echo yes || echo no) remotes=$(git -C "$PUB" remote | wc -l | tr -d ' ')" "ran=no remotes=0"
ok "an address git could read as an option is refused" "$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":\"--upload-pack=touch x\"}" | err)" "isn't a repository address"
ok "a token written into an https address is refused" "$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":\"https://me:ghp_x@github.com/me/app.git\"}" | err)" "token out of the address"
OUT=$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":$(jstr "$SB/nothing-here.git")}")
ok "an address with no repository behind it is checked first and not saved" "saved=$(printf '%s' "$OUT" | v "v['saved']") remotes=$(git -C "$PUB" remote | wc -l | tr -d ' ')" "saved=False remotes=0"
ok "  saying there is no repository there" "$(printf '%s' "$OUT" | v "v['problem']")" "no repository at this address"
FULL="$SB/publish-full.git"; git init -q --bare "$FULL"; git -C "$PUB" push -q "$FULL" main:dev 2>/dev/null
OUT=$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":$(jstr "$FULL")}")
ok "a repository that already has branches is not saved without the person's word" "saved=$(printf '%s' "$OUT" | v "v['saved']") branches=$(printf '%s' "$OUT" | v "','.join(v['existing_branches'])") remotes=$(git -C "$PUB" remote | wc -l | tr -d ' ')" "saved=False branches=dev remotes=0"
SAME="$SB/publish-same.git"; git init -q --bare "$SAME"; git -C "$PUB" push -q "$SAME" main:main 2>/dev/null
OUT=$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":$(jstr "$SAME")}")
ok "  and one that has this branch already is refused, with nothing to publish anyway" "$(printf '%s' "$OUT" | v "str(v['saved']) + ' ' + str(len(v['existing_branches'])) + ' ' + str('would be refused' in v['problem'])")" "False 0 True"
PUB2="$SB/publish2"; mkdir -p "$PUB2"
git init -q -b main "$PUB2" && printf 'z\n' > "$PUB2/c.txt" && git -C "$PUB2" add -A && git -C "$PUB2" commit -qm "first" >/dev/null
OUT=$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB2"),\"url\":$(jstr "$FULL"),\"allowExisting\":true}")
ok "  on the person's word it is saved" "saved=$(printf '%s' "$OUT" | v "v['saved']") | $(git -C "$PUB2" remote get-url origin)" "saved=True | $FULL"
OUT=$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":$(jstr "$BARE")}")
ok "the address of an empty repository is added as origin" "saved=$(printf '%s' "$OUT" | v "v['saved']") | $(git -C "$PUB" remote get-url origin)" "saved=True | $BARE"
OUT=$(pinvoke changes_push "{\"repoPath\":$(jstr "$PUB"),\"setUpstream\":true}")
ok "  then Publish lands there" "$(printf '%s' "$OUT" | v "v['ok']") $(git -C "$BARE" rev-parse main 2>/dev/null)" "True $(git -C "$PUB" rev-parse HEAD)"
ok "  and main tracks origin/main from now on" "upstream=$(git -C "$PUB" rev-parse --abbrev-ref 'main@{upstream}')" "upstream=origin/main"
ok "a repository that has a remote is never changed here" "$(pinvoke changes_add_remote "{\"repoPath\":$(jstr "$PUB"),\"url\":\"git@github.com:me/other.git\"}" | err | grep -c 'already has a remote') | $(git -C "$PUB" remote get-url origin)" "1 | $BARE"

section "Change the address of a remote that was never reached"
CHG="$SB/change"; mkdir -p "$CHG"
git init -q -b main "$CHG" && printf 'y\n' > "$CHG/b.txt" && git -C "$CHG" add -A && git -C "$CHG" commit -qm "first" >/dev/null
git -C "$CHG" remote add origin "$SB/typo.git"
CHGBARE="$SB/change-remote.git"; git init -q --bare "$CHGBARE"
OUT=$(pinvoke changes_push "{\"repoPath\":$(jstr "$CHG"),\"setUpstream\":true}")
ok "a push to an address with no repository says so" "$(printf '%s' "$OUT" | v "str(v['ok']) + ' ' + str('no repository at this address' in v['advice']['guidance'])")" "False True"
ok "  and offers to change the address of that remote, never reached" "$(printf '%s' "$OUT" | v "str(v['advice']['action']) + ' ' + str(v['advice']['remote'])")" "change-remote origin"
CUR=$(git -C "$CHG" remote get-url origin)
ok "an address that changed meanwhile is never overwritten" "$(pinvoke changes_change_remote "{\"repoPath\":$(jstr "$CHG"),\"remote\":\"origin\",\"from\":\"git@github.com:someone/else.git\",\"url\":$(jstr "$CHGBARE")}" | err) | $(git -C "$CHG" remote get-url origin)" "changed meanwhile"
ok "  (still the old one)" "$(git -C "$CHG" remote get-url origin)" "$CUR"
OUT=$(pinvoke changes_change_remote "{\"repoPath\":$(jstr "$CHG"),\"remote\":\"origin\",\"from\":$(jstr "$CUR"),\"url\":$(jstr "$SB/typo-again.git")}")
ok "a new address with no repository behind it is not saved" "saved=$(printf '%s' "$OUT" | v "v['saved']") | $(git -C "$CHG" remote get-url origin)" "saved=False | $CUR"
OUT=$(pinvoke changes_change_remote "{\"repoPath\":$(jstr "$CHG"),\"remote\":\"origin\",\"from\":$(jstr "$CUR"),\"url\":$(jstr "$CHGBARE")}")
ok "an empty repository's address replaces it" "saved=$(printf '%s' "$OUT" | v "v['saved']") | $(git -C "$CHG" remote get-url origin)" "saved=True | $CHGBARE"
OUT=$(pinvoke changes_push "{\"repoPath\":$(jstr "$CHG"),\"setUpstream\":true}")
ok "  and Publish lands there" "$(printf '%s' "$OUT" | v "v['ok']") $(git -C "$CHGBARE" rev-parse main 2>/dev/null)" "True $(git -C "$CHG" rev-parse HEAD)"
ok "a remote that has been pushed to is never changed here" "$(pinvoke changes_change_remote "{\"repoPath\":$(jstr "$CHG"),\"remote\":\"origin\",\"from\":$(jstr "$CHGBARE"),\"url\":$(jstr "$BARE")}" | err)" "has been fetched from or pushed to"
ok "  (still the address it was pushed to)" "$(git -C "$CHG" remote get-url origin)" "$CHGBARE"

section "Create the repository on GitHub, then publish (a fake GitHub; github.com is never reached)"
"$PY" - "$SB/api.log" > "$SB/api-port" <<'PYAPI' &
import http.server, json, os, sys
log = sys.argv[1]
SCOPES = {"t-me": "repo, read:org", "t-del": "repo, delete_repo, read:org", "t-granted": "repo, delete_repo", "t-wrong": "repo, delete_repo"}
LOGINS = {"t-me": "me", "t-del": "del", "t-granted": "me", "t-wrong": "someone-else"}
REPOS = {"me/made-here": {"id": 101, "private": True}, "del/old": {"id": 202, "private": False}, "me/second": {"id": 303, "private": True},
         "me/forked": {"id": 404, "private": False, "fork": True, "parent": "up/app"}}
class H(http.server.BaseHTTPRequestHandler):
    def answer(self, status, body, token=""):
        data = json.dumps(body).encode() if body is not None else b""
        self.send_response(status)
        self.send_header("X-OAuth-Scopes", SCOPES.get(token, "repo, read:org"))
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def repo(self, full):
        r = REPOS[full]
        owner = full.split("/")[0]
        out = {"id": r["id"], "full_name": full, "private": r["private"], "visibility": "private" if r["private"] else "public", "fork": r.get("fork", False), "archived": False,
               "owner": {"login": owner, "type": "User"}, "permissions": {"admin": True}, "stargazers_count": 2, "subscribers_count": 1, "forks_count": 0, "has_pages": False}
        if r.get("parent"):
            out["parent"] = {"full_name": r["parent"]}
        return out
    def handle_any(self, method):
        token = self.headers.get("Authorization", "").split(" ")[-1]
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        path = self.path.split("?")[0]
        with open(log, "a") as f:
            f.write(json.dumps({"m": method, "p": path, "t": token, "b": raw}) + "\n")
        # The device flow (GITSWITCH_GITHUB_WEB): a code, then a token for it.
        if method == "POST" and path == "/login/device/code":
            return self.answer(200, {"device_code": "dc-1", "user_code": "ABCD-1234", "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 1})
        if method == "POST" and path == "/login/oauth/access_token":
            wrong = os.path.exists(log + ".wrong")
            return self.answer(200, {"access_token": "t-wrong" if wrong else "t-granted", "token_type": "bearer", "scope": "repo,delete_repo"})
        if token not in SCOPES:
            return self.answer(401, {"message": "Bad credentials"})
        body = json.loads(raw) if raw else {}
        if method == "GET" and path == "/user":
            return self.answer(200, {"login": LOGINS[token]}, token)
        parts = path.strip("/").split("/")
        if len(parts) >= 3 and parts[0] == "repos":
            full = parts[1] + "/" + parts[2]
            if full not in REPOS:
                return self.answer(404, {"message": "Not Found"}, token)
            if method == "GET" and len(parts) == 3:
                return self.answer(200, self.repo(full), token)
            if method == "GET" and len(parts) == 4 and parts[3] == "branches":
                return self.answer(200, [{"name": "main", "commit": {"sha": "1" * 40}}], token)
            if method == "PATCH" and len(parts) == 3:
                REPOS[full]["private"] = body.get("visibility") == "private"
                return self.answer(200, self.repo(full), token)
            if method == "DELETE" and len(parts) == 3:
                if "delete_repo" not in SCOPES[token]:
                    return self.answer(403, {"message": "Must have admin rights to Repository."}, token)
                del REPOS[full]
                return self.answer(204, None, token)
        if token != "t-me":
            return self.answer(404, {"message": "Not Found"}, token)
        def made(owner):
            name = body.get("name", "")
            return {"full_name": owner + "/" + name, "ssh_url": "git@github.com:" + owner + "/" + name + ".git", "private": body.get("private", False), "size": 0}
        if method == "GET" and path == "/user/orgs":
            return self.answer(200, [{"login": "acme"}])
        if method == "POST" and path == "/user/repos":
            if body.get("name") == "taken":
                return self.answer(422, {"message": "Repository creation failed.", "errors": [{"resource": "Repository", "code": "custom", "field": "name", "message": "name already exists on this account"}]})
            return self.answer(201, made("me"))
        if method == "POST" and path == "/orgs/acme/repos":
            return self.answer(201, made("acme"))
        if method == "POST" and path == "/orgs/closed/repos":
            return self.answer(403, {"message": "Although you appear to have the correct authorization credentials, the `closed` organization has enabled OAuth App access restrictions, meaning that data access to third-parties is limited."})
        return self.answer(404, {"message": "Not Found"})
    def do_GET(self): self.handle_any("GET")
    def do_POST(self): self.handle_any("POST")
    def do_PATCH(self): self.handle_any("PATCH")
    def do_DELETE(self): self.handle_any("DELETE")
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
PYAPI
API_PID=$!
trap '{ kill "$API_PID" && wait "$API_PID"; } 2>/dev/null' EXIT
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/api-port" ] && break; sleep 0.5; done
if [ ! -s "$SB/api-port" ] || [ -z "${GITSWITCH_TOKEN_DIR:-}" ]; then
  echo "  SKIPPED: the fake GitHub didn't start (or no sandboxed token store), so nothing is asked of GitHub"
else
  export GITSWITCH_GITHUB_API="http://127.0.0.1:$(cat "$SB/api-port")"
  export GITSWITCH_GITHUB_WEB="$GITSWITCH_GITHUB_API"
  mkdir -p "$GITSWITCH_TOKEN_DIR"
  printf '[{"login":"me","id":1},{"login":"del","id":2}]' > "$GITSWITCH_TOKEN_DIR/github-signins.json"
  printf 't-me' > "$GITSWITCH_TOKEN_DIR/github_me"
  printf 't-del' > "$GITSWITCH_TOKEN_DIR/github_del"
  # Every GitHub address leads to a folder that isn't there, so nothing here
  # can reach github.com; each repository the fake makes gets a local bare one.
  git config --global "url.$SB/no-github/.insteadOf" "git@github.com:"
  git config --global --add "url.$SB/no-github/.insteadOf" "https://github.com/"
  bare_for() { local b="$SB/gh-$1-$2.git"; git init -q --bare "$b"; git config --global "url.$b.insteadOf" "git@github.com:$1/$2.git"; printf '%s' "$b"; }
  fresh() { local d="$SB/$1"; mkdir -p "$d"; git init -q -b main "$d" && printf '%s\n' "$1" > "$d/f.txt" && git -C "$d" add -A && git -C "$d" commit -qm "first" >/dev/null; printf '%s' "$d"; }
  posts() { grep -c '"m": "POST"' "$SB/api.log" 2>/dev/null || true; }
  lastpost() { grep '"m": "POST"' "$SB/api.log" | tail -n 1 | "$PY" -c 'import sys,json; r=json.loads(sys.stdin.read()); b=json.loads(r["b"] or "{}"); print(eval(sys.argv[1], {"r": r, "b": b}))' "$1"; }
  create() { pinvoke changes_create_github_repo "$1" | tee -a "$SB/answers.log"; }
  as_me() { printf '{"repoPath":%s,"account":"signin:me",%s}' "$(jstr "$1")" "$2"; }

  NEW=$(fresh "made here")
  OUT=$(pinvoke github_publish_context "{\"repoPath\":$(jstr "$NEW")}" | tee -a "$SB/answers.log")
  ok "the form starts from the account, the target and the folder's name" "$(printf '%s' "$OUT" | v "' '.join(a['account'] for a in v['accounts']) + ' ' + v['target']['kind'] + ' ' + v['default_name'] + ' ' + str(v['branch']) + ' ' + str(v['blocked'])")" "signin:me signin:del add made-here main None"
  OUT=$(pinvoke github_publish_accounts "{\"repoPath\":$(jstr "$NEW")}")
  ok "  the dialog's accounts come from local files, nothing asked of GitHub" "$(printf '%s' "$OUT" | v "' '.join(a['account'] for a in v['accounts'])") gets=$( (cat "$SB/api.log" 2>/dev/null || true) | grep -c '"m": "GET"')" "signin:me signin:del gets=0"
  # gh's own file names its logins: read without starting gh's network check.
  mkdir -p "$GH_CONFIG_DIR"
  printf 'github.com:\n    users:\n        alice:\n            oauth_token: gho_never_read\n    user: alice\n' > "$GH_CONFIG_DIR/hosts.yml"
  OUT=$(pinvoke github_publish_accounts "{\"repoPath\":$(jstr "$NEW")}")
  if command -v gh >/dev/null 2>&1 || [ -x "$(command -v gh.exe 2>/dev/null)" ]; then
    ok "  a GitHub CLI login in gh's hosts.yml is listed, its token never read" "$(printf '%s' "$OUT" | v "' '.join(a['account'] for a in v['accounts'])") | $(printf '%s' "$OUT" | grep -c gho_never_read)" "gh:alice signin:me signin:del | 0"
  else
    ok "  without a gh to hand out tokens, hosts.yml's logins are not listed" "$(printf '%s' "$OUT" | v "' '.join(a['account'] for a in v['accounts'])")" "signin:me signin:del"
  fi
  rm -f "$GH_CONFIG_DIR/hosts.yml"
  OUT=$(pinvoke github_owners '{"account":"signin:me"}' | tee -a "$SB/answers.log")
  ok "  owners: the account, then its organizations" "$(printf '%s' "$OUT" | v "' '.join(o['login'] + ':' + o['kind'] for o in v['owners'])")" "me:user acme:org"
  B1=$(bare_for me made-here)
  OUT=$(create "$(as_me "$NEW" '"owner":"me","name":"made-here","description":"From  the\nsuite","private":true,"target":{"kind":"add"}')")
  ok "Create makes it on GitHub and connects it as origin" "$(printf '%s' "$OUT" | v "v['saved'] + ' ' + v['full_name'] + ' [' + v['problem'] + ']'") | $(git -C "$NEW" config --get remote.origin.url)" "added me/made-here [] | git@github.com:me/made-here.git"
  ok "  asked as POST /user/repos: private, empty (no auto_init), the description on one line" "$(lastpost 'r["p"] + " " + str(b.get("private")) + " " + str(b.get("auto_init")) + " " + str(b.get("description"))')" "/user/repos True False From the suite"
  OUT=$(pinvoke changes_push "{\"repoPath\":$(jstr "$NEW"),\"setUpstream\":true}")
  ok "  then Publish lands in it" "$(printf '%s' "$OUT" | v "v['ok']") $(git -C "$B1" rev-parse main 2>/dev/null)" "True $(git -C "$NEW" rev-parse HEAD)"

  NEW2=$(fresh taken-folder)
  OUT=$(create "$(as_me "$NEW2" '"owner":"me","name":"taken","description":"","private":true,"target":{"kind":"add"}')")
  ok "a name already taken is GitHub's refusal, worded" "$(printf '%s' "$OUT" | err)" "There's already a repository me/taken"
  ok "  and nothing was saved" "remotes=$(git -C "$NEW2" remote | wc -l | tr -d ' ')" "remotes=0"

  NEW3=$(fresh org-folder); B3=$(bare_for acme tool)
  OUT=$(create "$(as_me "$NEW3" '"owner":"acme","name":"tool","description":"","private":false,"target":{"kind":"add"}')")
  ok "an organization's repository is made through the organization's endpoint" "$(printf '%s' "$OUT" | v "v['saved'] + ' ' + v['full_name']") $(lastpost 'r["p"] + " private=" + str(b.get("private"))')" "added acme/tool /orgs/acme/repos private=False"

  NEW4=$(fresh closed-folder)
  OUT=$(create "$(as_me "$NEW4" '"owner":"closed","name":"x","description":"","private":true,"target":{"kind":"add"}')")
  ok "an organization that restricts third-party apps is named, with the way out" "$(printf '%s' "$OUT" | err)" "restricts third-party apps"

  CH=$(fresh change-folder); git -C "$CH" remote add origin "git@github.com:me/typo.git"; BF=$(bare_for me fixed)
  OUT=$(create "$(as_me "$CH" '"owner":"me","name":"fixed","description":"","private":true,"target":{"kind":"change","remote":"origin","from":"'"$(git -C "$CH" remote get-url origin)"'"}')")
  ok "a remote never reached gets the new repository's address" "$(printf '%s' "$OUT" | v "v['saved'] + ' [' + v['problem'] + ']'") | $(git -C "$CH" config --get remote.origin.url)" "changed [] | git@github.com:me/fixed.git"

  N=$(posts)
  ok "a folder whose remote has been pushed to is refused before GitHub is asked" "$(create "$(as_me "$PUB" '"owner":"me","name":"again","description":"","private":true,"target":{"kind":"add"}')" | err | grep -c 'already has a remote') posts+$(( $(posts) - N ))" "1 posts+0"
  UNBORN="$SB/unborn-folder"; mkdir -p "$UNBORN"; git init -q -b main "$UNBORN"
  N=$(posts)
  ok "a folder with no commits is refused before GitHub is asked" "$(create "$(as_me "$UNBORN" '"owner":"me","name":"empty","description":"","private":true,"target":{"kind":"add"}')" | err | grep -c 'NO ERROR') posts+$(( $(posts) - N ))" "0 posts+0"

  # Who can see it, and deleting it (the same fake; nothing reaches github.com).
  facts() { pinvoke github_repo_facts "$1" | tee -a "$SB/answers.log"; }
  OUT=$(facts "{\"account\":\"signin:me\",\"owner\":\"me\",\"name\":\"made-here\",\"repoPath\":$(jstr "$NEW")}")
  ok "a repository's facts: its id, private, admin, no delete permission yet, a branch the folder lacks" "$(printf '%s' "$OUT" | v "str(v['id']) + ' ' + str(v['private']) + ' ' + str(v['admin']) + ' ' + str(v['can_delete']) + ' ' + ','.join(v['missing_here'])")" "101 True True False main"
  vis() { pinvoke github_set_visibility "$1" | tee -a "$SB/answers.log"; }
  patches() { grep -c '"m": "PATCH"' "$SB/api.log" 2>/dev/null || true; }
  deletes() { grep -c '"m": "DELETE"' "$SB/api.log" 2>/dev/null || true; }
  N=$(patches)
  OUT=$(vis '{"account":"signin:me","owner":"me","name":"made-here","private":false,"expectId":999}')
  ok "another repository under the same name is never changed" "$(printf '%s' "$OUT" | err | grep -c 'no longer the repository') patches+$(( $(patches) - N ))" "1 patches+0"
  N=$(patches)
  OUT=$(vis '{"account":"signin:me","owner":"me","name":"forked","private":true,"expectId":404}')
  ok "a fork's visibility is never changed: GitHub doesn't let it, whichever way" "$(printf '%s' "$OUT" | err | grep -c "is a fork of up/app, and GitHub doesn't let") patches+$(( $(patches) - N ))" "1 patches+0"
  OUT=$(vis '{"account":"signin:me","owner":"me","name":"made-here","private":false,"expectId":101}')
  ok "Make public changes it, sending only the visibility" "$(printf '%s' "$OUT" | v "v['visibility']") $(grep '"m": "PATCH"' "$SB/api.log" | tail -n 1 | "$PY" -c 'import sys,json; print(json.loads(sys.stdin.read())["b"])')" 'public {"visibility":"public"}'
  del() { pinvoke github_delete_repo "$1" | tee -a "$SB/answers.log"; }
  N=$(deletes)
  OUT=$(del '{"account":"signin:me","owner":"me","name":"made-here","confirm":"me/made-her","expectId":101}')
  ok "a delete without the name typed sends nothing" "$(printf '%s' "$OUT" | err | grep -c 'type me/made-here') deletes+$(( $(deletes) - N ))" "1 deletes+0"
  OUT=$(del '{"account":"signin:me","owner":"me","name":"made-here","confirm":"  ME/made-here ","expectId":101}')
  ok "  without GitHub's delete permission it asks for the approval, and sends no delete" "$(printf '%s' "$OUT" | v "v['outcome']") deletes+$(( $(deletes) - N ))" "needs-permission deletes+0"
  touch "$SB/api.log.wrong"
  OUT=$(pinvoke github_delete_grant_flow '{"account":"signin:me","owner":"me","name":"made-here","confirm":"me/made-here","expectId":101}' | tee -a "$SB/answers.log")
  rm -f "$SB/api.log.wrong"
  ok "an approval given as another account is refused, and nothing is deleted" "deletes+$(( $(deletes) - N )) $(printf '%s' "$OUT" | v "str(v['outcome']) + ' ' + str(v['waited'].get('Err', ''))")" "deletes+0 None Command execution error: That was approved as @someone-else, not @me. Nothing was deleted"
  ok "  the approval asks GitHub for exactly repo and delete_repo" "$(grep '/login/device/code' "$SB/api.log" | tail -n 1 | "$PY" -c 'import sys,json,urllib.parse; print(urllib.parse.parse_qs(json.loads(sys.stdin.read())["b"])["scope"][0])')" "repo delete_repo"
  OUT=$(pinvoke github_delete_grant_flow '{"account":"signin:me","owner":"me","name":"made-here","confirm":"me/made-here","expectId":101}' | tee -a "$SB/answers.log")
  ok "with the approval, it is deleted on GitHub (the code was shown first)" "$(printf '%s' "$OUT" | v "v['user_code'] + ' ' + v['outcome']['Ok']['outcome']") | $(grep '"m": "DELETE"' "$SB/api.log" | tail -n 1 | "$PY" -c 'import sys,json; r=json.loads(sys.stdin.read()); print(r["p"], r["t"])')" "ABCD-1234 deleted | /repos/me/made-here t-granted"
  ok "  and the folder on this computer is untouched" "$(git -C "$NEW" rev-parse HEAD)" "$(git -C "$B1" rev-parse main)"
  OUT=$(del '{"account":"signin:del","owner":"del","name":"old","confirm":"del/old","expectId":202}')
  ok "an account whose token has delete_repo deletes at once, and is told about the 90 days" "$(printf '%s' "$OUT" | v "v['outcome'] + ' ' + str('90 days' in v['message'])")" "deleted True"
  # Made again where the folder's address points, after GitHub lost it (the
  # one deleted above): that remote was pushed to, and its address never moves.
  SEEN=$(git -C "$NEW" remote get-url origin)   # as the status shows it: git applies the suite's insteadOf
  OUT=$(pinvoke github_publish_context "{\"repoPath\":$(jstr "$NEW")}")
  ok "the dialog reads that remote as reached, naming its repository from the address as configured" "$(printf '%s' "$OUT" | v "v['target']['kind'] + ' ' + str(v['target']['remote']) + ' ' + str(v['target']['repo'])")" "none origin me/made-here"
  N=$(posts)
  OUT=$(create "$(as_me "$NEW" '"owner":"me","name":"other","description":"","private":true,"target":{"kind":"again","remote":"origin","from":'"$(jstr "$SEEN")"'}')")
  ok "made again only under the name the address has: another name is refused before GitHub is asked" "$(printf '%s' "$OUT" | err | grep -c 'names me/made-here, not me/other') posts+$(( $(posts) - N ))" "1 posts+0"
  OUT=$(create "$(as_me "$NEW" '"owner":"me","name":"made-here","description":"","private":true,"target":{"kind":"again","remote":"origin","from":"git@github.com:me/old.git"}')")
  ok "  an address that changed meanwhile is refused too" "$(printf '%s' "$OUT" | err | grep -c 'changed meanwhile') posts+$(( $(posts) - N ))" "1 posts+0"
  OUT=$(create "$(as_me "$NEW" '"owner":"me","name":"made-here","description":"","private":true,"target":{"kind":"change","remote":"origin","from":'"$(jstr "$SEEN")"'}')")
  ok "  changing that remote's address stays refused (it was pushed to)" "$(printf '%s' "$OUT" | err | grep -c 'has been fetched from or pushed to') posts+$(( $(posts) - N ))" "1 posts+0"
  OUT=$(create "$(as_me "$NEW" '"owner":"me","name":"made-here","description":"","private":true,"target":{"kind":"again","remote":"origin","from":'"$(jstr "$SEEN")"'}')")
  ok "made again on GitHub under the same name: the address is kept as it was" "$(printf '%s' "$OUT" | v "v['saved'] + ' ' + v['full_name'] + ' ' + v['url'] + ' [' + v['problem'] + ']'") | $(git -C "$NEW" config --get remote.origin.url) | $(lastpost 'r["p"] + " " + b.get("name") + " private=" + str(b.get("private"))')" "kept me/made-here git@github.com:me/made-here.git [] | git@github.com:me/made-here.git | /user/repos made-here private=True"
  TK=$(fresh taken-again); git -C "$TK" remote add origin "git@github.com:me/taken.git"; git -C "$TK" update-ref refs/remotes/origin/main HEAD
  N=$(posts)
  OUT=$(create "$(as_me "$TK" '"owner":"me","name":"taken","description":"","private":true,"target":{"kind":"again","remote":"origin","from":'"$(jstr "$(git -C "$TK" remote get-url origin)")"'}')")
  ok "  made again while GitHub still has it: said so, the folder untouched" "$(printf '%s' "$OUT" | err | grep -c 'is still on GitHub, so nothing was created') posts+$(( $(posts) - N )) | $(git -C "$TK" config --get remote.origin.url)" "1 posts+1 | git@github.com:me/taken.git"
  # GitHub has it and the folder reaches it (restored on github.com, say): push to it, nothing to make.
  bare_for me taken >/dev/null
  TR=$(fresh taken-reached); git -C "$TR" remote add origin "git@github.com:me/taken.git"; git -C "$TR" update-ref refs/remotes/origin/main HEAD
  N=$(posts)
  OUT=$(create "$(as_me "$TR" '"owner":"me","name":"taken","description":"","private":true,"target":{"kind":"again","remote":"origin","from":'"$(jstr "$(git -C "$TR" remote get-url origin)")"'}')")
  ok "  made again while GitHub has it and this folder reaches it (restored, say): said so, push to it, the folder untouched" "$(printf '%s' "$OUT" | err | grep -c 'is on GitHub, and this folder can reach it, so nothing was created. Close this and push to it.') posts+$(( $(posts) - N )) | $(git -C "$TR" config --get remote.origin.url)" "1 posts+1 | git@github.com:me/taken.git"
  ok "no answer carries a token" "$(grep -cE 't-me|t-del|t-granted|t-wrong' "$SB/answers.log")" "0"
  ok "no answer carries the account's token" "$(grep -c 't-me' "$SB/answers.log")" "0"
fi

verify_result
