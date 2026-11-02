#!/bin/bash
# GitHub from inside GitSwitch, end to end against the real backend: which of
# your accounts can act on a clone's repository (the profile's own first),
# pull requests listed and filtered (open, merged, closed, all, and a search
# across repositories), one pull request read whole (merge options, reviews,
# checks, files with their diffs, commits), a review sent, a merge with the
# commit that was reviewed (and the branch deleted only when it is safe), and
# collaborators listed, invited and removed — against a fake GitHub and a fake
# gh. Every request must carry the token of the account that was picked, and
# no token may ever reach an answer.
set -u
source "$(dirname "$0")/lib.sh"
verify_init github "GitHub Tester" "gh@example.test"
SB="$(realdir "$SB")"
# The profile store follows HOME on macOS and Linux, not on Windows: there the
# store would be the real one. The request logic is covered there by the unit tests.
if command -v cygpath >/dev/null 2>&1; then
  echo "github suite: skipped on Windows (the profile store does not follow HOME there)"
  verify_result
  exit $?
fi
unset XDG_DATA_HOME XDG_CONFIG_HOME GH_TOKEN GITHUB_TOKEN
H="$(realdir "$HOME")"; export HOME="$H"

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
export PATH="$GHB:$PATH"

"$PY" - "$SB/api.log" > "$SB/api-port" <<'PYAPI' &
import http.server, json, sys, urllib.parse
log = sys.argv[1]
# Who each token is, and what it may do on acme/app.
TOKENS = {"t-me-home": "me-home", "t-work-me": "work-me"}
PERM = {"t-me-home": {"admin": False, "push": False, "pull": True}, "t-work-me": {"admin": True, "push": True, "pull": True}}
HEAD = "abc1234def5678"
def pull(n, state, merged, extra=None):
    p = {"number": n, "title": f"PR {n}", "user": {"login": "me-home"}, "state": state, "draft": False,
         "merged_at": "2026-10-01T00:00:00Z" if merged else None, "closed_at": None if state == "open" else "2026-10-01T00:00:00Z",
         "created_at": "2026-09-01T00:00:00Z", "updated_at": "2026-10-02T00:00:00Z", "labels": [],
         "head": {"ref": f"feat/{n}", "sha": HEAD, "repo": {"full_name": "acme/app"}}, "base": {"ref": "main"}, "html_url": f"https://github.com/acme/app/pull/{n}"}
    p.update(extra or {})
    return p
OPEN = [pull(1, "open", False), pull(2, "open", False, {"draft": True})]
# Closed: 60 over two pages, every third merged.
CLOSED = [pull(100 + i, "closed", i % 3 == 0) for i in range(60)]
STATE = {"merged": False, "deleted": [], "collabs": [{"login": "work-me", "role_name": "admin"}, {"login": "pal", "role_name": "write"}],
         "invites": [{"id": 77, "invitee": {"login": "newbie"}, "permissions": "write", "created_at": "2026-10-30T00:00:00Z", "inviter": {"login": "work-me"}}]}
class H(http.server.BaseHTTPRequestHandler):
    def answer(self, status, body=None, headers=None):
        data = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("X-OAuth-Scopes", "repo, read:org")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(data))); self.end_headers()
        self.wfile.write(data)
    def handle_any(self, method):
        token = self.headers.get("Authorization", "").split(" ")[-1]
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        url = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(url.query).items()}
        with open(log, "a") as f:
            f.write(json.dumps({"m": method, "p": url.path, "q": q, "t": token, "b": body}) + "\n")
        if token not in TOKENS:
            return self.answer(401, {"message": "Bad credentials"})
        p = url.path
        me = TOKENS[token]
        base = f"http://127.0.0.1:{self.server.server_address[1]}"
        if method == "GET" and p == "/repos/acme/app":
            return self.answer(200, {"full_name": "acme/app", "private": True, "default_branch": "main", "permissions": PERM[token],
                                     **({"allow_merge_commit": True, "allow_squash_merge": True, "allow_rebase_merge": False} if PERM[token]["admin"] else {})})
        if method == "GET" and p == "/repos/acme/app/pulls":
            st, page, per = q.get("state"), int(q.get("page", 1)), int(q.get("per_page", 30))
            src = OPEN if st == "open" else CLOSED if st == "closed" else OPEN + CLOSED
            if STATE["merged"] and st == "open":
                src = [x for x in OPEN if x["number"] != 1]
            chunk = src[(page - 1) * per: page * per]
            link = {"Link": f'<{base}/repos/acme/app/pulls?page={page + 1}>; rel="next"'} if page * per < len(src) else {}
            return self.answer(200, chunk, link)
        if method == "GET" and p == "/search/issues":
            items = [{"number": 9, "title": "Elsewhere", "user": {"login": "pal"}, "state": "open", "comments": 2, "labels": [{"name": "review"}],
                      "repository_url": "https://api.github.com/repos/other/lib", "pull_request": {"merged_at": None}, "html_url": "h",
                      "created_at": "2026-10-01T00:00:00Z", "updated_at": "2026-10-03T00:00:00Z"}]
            return self.answer(200, {"total_count": 1, "items": items})
        if method == "GET" and p == "/repos/acme/app/pulls/1":
            return self.answer(200, pull(1, "closed" if STATE["merged"] else "open", STATE["merged"], {"body": "Adds the thing.", "merged": STATE["merged"], "mergeable": True, "mergeable_state": "clean",
                                                    "additions": 3, "deletions": 1, "changed_files": 2, "commits": 2, "comments": 1, "review_comments": 0}))
        if method == "GET" and p == "/repos/acme/app/pulls/1/reviews":
            return self.answer(200, [{"user": {"login": "pal"}, "state": "APPROVED", "submitted_at": "2026-10-02T00:00:00Z"}])
        if method == "GET" and p == f"/repos/acme/app/commits/{HEAD}/check-runs":
            return self.answer(200, {"check_runs": [{"name": "build", "status": "completed", "conclusion": "success", "html_url": "u"}]})
        if method == "GET" and p == f"/repos/acme/app/commits/{HEAD}/status":
            return self.answer(200, {"statuses": []})
        if method == "GET" and p == "/repos/acme/app/pulls/1/files":
            return self.answer(200, [{"filename": "src/a.txt", "status": "modified", "additions": 1, "deletions": 1, "patch": "@@ -1 +1 @@\n-old\n+new"},
                                     {"filename": "logo.png", "status": "added", "additions": 0, "deletions": 0}])
        if method == "GET" and p == "/repos/acme/app/pulls/1/commits":
            return self.answer(200, [{"sha": "c1", "commit": {"message": "first\n\nbody", "author": {"name": "Me", "date": "2026-10-01T00:00:00Z"}}, "author": {"login": "me-home"}},
                                     {"sha": "c2", "commit": {"message": "second", "author": {"name": "Someone", "date": "2026-10-02T00:00:00Z"}}, "author": None}])
        if method == "POST" and p == "/repos/acme/app/pulls/1/reviews":
            ev = json.loads(body).get("event")
            if me == "me-home" and ev in ("APPROVE", "REQUEST_CHANGES"):
                return self.answer(422, {"message": "Unprocessable Entity", "errors": ["Can not approve your own pull request"]})
            return self.answer(200, {"user": {"login": me}, "state": {"APPROVE": "APPROVED", "REQUEST_CHANGES": "CHANGES_REQUESTED", "COMMENT": "COMMENTED"}[ev], "submitted_at": "x"})
        if method == "PUT" and p == "/repos/acme/app/pulls/1/merge":
            b = json.loads(body)
            if not PERM[token]["push"]:
                return self.answer(403, {"message": "Must have push access"})
            if b.get("sha") != HEAD:
                return self.answer(409, {"message": "Head branch was modified. Review and try the merge again."})
            STATE["merged"] = True
            return self.answer(200, {"merged": True, "sha": "m3rg3d", "message": "Pull Request successfully merged"})
        if method == "DELETE" and p.startswith("/repos/acme/app/git/refs/heads/"):
            STATE["deleted"].append(p.split("/heads/", 1)[1])
            return self.answer(204)
        if method == "GET" and p == "/repos/acme/app/collaborators":
            if not PERM[token]["push"]:
                return self.answer(403, {"message": "Must have push access to view repository collaborators."})
            return self.answer(200, STATE["collabs"])
        if method == "GET" and p == "/repos/acme/app/invitations":
            return self.answer(200, STATE["invites"]) if PERM[token]["admin"] else self.answer(403, {"message": "Forbidden"})
        if method == "GET" and p.startswith("/users/"):
            u = p.split("/")[2]
            return self.answer(404, {"message": "Not Found"}) if u == "ghost-user" else self.answer(200, {"login": u.capitalize() if u == "friend" else u})
        if method == "PUT" and p.startswith("/repos/acme/app/collaborators/"):
            u = p.rsplit("/", 1)[1]
            if any(c["login"] == u for c in STATE["collabs"]):
                return self.answer(204)
            STATE["invites"].append({"id": 78, "invitee": {"login": u}, "permissions": json.loads(body)["permission"], "created_at": "x", "inviter": {"login": me}})
            return self.answer(201, {"id": 78})
        if method == "DELETE" and p.startswith("/repos/acme/app/collaborators/"):
            u = p.rsplit("/", 1)[1]
            STATE["collabs"] = [c for c in STATE["collabs"] if c["login"] != u]
            return self.answer(204)
        if method == "DELETE" and p.startswith("/repos/acme/app/invitations/"):
            i = int(p.rsplit("/", 1)[1])
            STATE["invites"] = [x for x in STATE["invites"] if x["id"] != i]
            return self.answer(204)
        return self.answer(404, {"message": "Not Found"})
    def do_GET(self): self.handle_any("GET")
    def do_POST(self): self.handle_any("POST")
    def do_PUT(self): self.handle_any("PUT")
    def do_DELETE(self): self.handle_any("DELETE")
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
PYAPI
API_PID=$!
trap '{ kill "$API_PID" && wait "$API_PID"; } 2>/dev/null' EXIT
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$SB/api-port" ] && break; sleep 0.5; done
export GITSWITCH_GITHUB_API="http://127.0.0.1:$(cat "$SB/api-port")"

# gh <command> <json>: the command's {"ok", "value"|"error"} answer.
call() { pinvoke "$1" "$2" | tee -a "$SB/answers.log"; }
val() { "$PY" -c 'import sys,json; d=json.load(sys.stdin); v=d.get("value") if d.get("ok") else {"error": d.get("error")}; print(eval(sys.argv[1], {"v": v}))' "$1"; }
lastreq() { tail -n "${2:-1}" "$SB/api.log" | head -n 1 | "$PY" -c 'import sys,json; r=json.loads(sys.stdin.read()); print(eval(sys.argv[1], {"r": r}))' "$1"; }
A='"account":"gh:work-me","owner":"acme","name":"app"'
# args '<more fields>': the JSON for acting as work-me on acme/app, plus those fields.
args() { if [ -n "${1:-}" ]; then printf '{%s,%s}' "$A" "$1"; else printf '{%s}' "$A"; fi; }

section "Which account acts on a clone's repository"
printf '%s' "$("$PY" -c 'import json,sys; print(json.dumps({"name":"Work","git_name":"Me At Work","git_email":"me@work.test","directories":[sys.argv[1]],"github_login":"work-me"}))' "$H/work")" > "$SB/p.json"
probe profile_apply "$SB" "$SB/p.json" >/dev/null
mkdir -p "$H/work" && git init -q "$H/work/app" && git -C "$H/work/app" remote add origin git@github.com:acme/app.git
OUT=$(call github_repo_context "$("$PY" -c 'import json,sys; print(json.dumps({"repoPath":sys.argv[1]}))' "$H/work/app")")
ok "the repository comes from the clone's remote" "$(printf '%s' "$OUT" | val 'v["owner"]+"/"+v["name"]')" "acme/app"
ok "  every account is asked what it may do there" "$(printf '%s' "$OUT" | val '[(a["account"], a["permission"]) for a in v["accounts"]]')" "[('gh:me-home', 'read'), ('gh:work-me', 'admin')]"
ok "  the profile's own login acts by default" "$(printf '%s' "$OUT" | val 'v["suggested"], v["profile_login"]')" "('gh:work-me', 'work-me')"
git init -q "$H/elsewhere" && git -C "$H/elsewhere" remote add origin https://gitlab.com/x/y.git
ok "a clone whose remote is not on GitHub says so" "$(call github_repo_context "$("$PY" -c 'import json,sys; print(json.dumps({"repoPath":sys.argv[1]}))' "$H/elsewhere")" | val 'v["not_github"]')" "Its remote (https://gitlab.com/x/y.git) is not on GitHub."
git init -q "$H/bare-local"
ok "  and one with no remote" "$(call github_repo_context "$("$PY" -c 'import json,sys; print(json.dumps({"repoPath":sys.argv[1]}))' "$H/bare-local")" | val 'v["not_github"]')" "This repository has no remote."

section "Pull requests: open, merged, closed, all, and a search across repositories"
ok "open: the open ones" "$(call github_pulls "$(args '"state":"open","page":1')" | val '[(i["number"], i["state"], i["draft"], i["head_ref"]) for i in v["items"]], v["next_page"]')" "([(1, 'open', False, 'feat/1'), (2, 'open', True, 'feat/2')], None)"
ok "  asked as the account picked" "$(lastreq 'r["t"]')" "t-work-me"
OUT=$(call github_pulls "$(args '"state":"merged","page":1')")
ok "merged: only merged ones, read on through closed pages until a page is full" "$(printf '%s' "$OUT" | val 'len(v["items"]), all(i["state"]=="merged" for i in v["items"]), v["next_page"]')" "(20, True, None)"
OUT=$(call github_pulls "$(args '"state":"closed","page":1')")
ok "closed: the ones closed without merging (a page read whole, the next where it stopped)" "$(printf '%s' "$OUT" | val 'len(v["items"]), all(i["state"]=="closed" and not i["merged_at"] for i in v["items"]), v["next_page"]')" "(33, True, 2)"
ok "all: one page, and where the next starts" "$(call github_pulls "$(args '"state":"all","page":1')" | val 'len(v["items"]), v["next_page"]')" "(30, 2)"
ok "  the next page carries on" "$(call github_pulls "$(args '"state":"all","page":2')" | val 'v["items"][0]["number"]')" "128"
OUT=$(call github_pulls_search '{"account":"gh:me-home","scope":"review_requested","state":"open","repo":null,"page":1}')
ok "a search across repositories: the row names its repository" "$(printf '%s' "$OUT" | val '[(i["repo"], i["number"], i["comments"], i["labels"]) for i in v["items"]], v["total"]')" "([('other/lib', 9, 2, ['review'])], 1)"
ok "  GitHub is asked for exactly that" "$(lastreq 'r["p"], r["q"]["q"], r["t"]')" "('/search/issues', 'is:pr review-requested:@me archived:false is:open', 't-me-home')"
ok "an unknown state is refused before GitHub is asked" "$(call github_pulls "$(args '"state":"everything","page":1')" | val 'v["error"]')" "Configuration error: Unknown pull request state \"everything\""
ok "a repository name that would reach another API path is refused" "$(call github_pulls '{"account":"gh:work-me","owner":"acme","name":"app/../../user","state":"open","page":1}' | val '"is not a GitHub repository name" in v["error"]')" "True"

section "One pull request, whole"
OUT=$(call github_pull "$(args '"number":1')")
ok "its state, branches and size" "$(printf '%s' "$OUT" | val 'v["state"], v["head_ref"], v["base_ref"], v["head_sha"], v["additions"], v["deletions"], v["changed_files"]')" "('open', 'feat/1', 'main', 'abc1234def5678', 3, 1, 2)"
ok "  what the repository allows for merging (an admin is told)" "$(printf '%s' "$OUT" | val 'v["merge_methods"], v["merge_methods_known"], v["can_push"], v["is_admin"]')" "(['merge', 'squash'], True, True, True)"
ok "  reviews and checks" "$(printf '%s' "$OUT" | val '[(r["login"], r["state"]) for r in v["reviews"]], [(c["name"], c["state"]) for c in v["checks"]]')" "([('pal', 'APPROVED')], [('build', 'success')])"
ok "  its branch can be deleted after merging: it lives here and is not the default" "$(printf '%s' "$OUT" | val 'v["can_delete_branch"]')" "True"
ok "  as someone who can only read: no merge options from GitHub, all offered" "$(call github_pull '{"account":"gh:me-home","owner":"acme","name":"app","number":1}' | val 'v["merge_methods"], v["merge_methods_known"], v["can_push"]')" "(['merge', 'squash', 'rebase'], False, False)"
OUT=$(call github_pull_files "$(args '"number":1')")
ok "its files, each with its diff" "$(printf '%s' "$OUT" | val '[(f["path"], f["diff"]["added"], f["diff"]["removed"], [l["kind"] for l in f["diff"]["lines"]]) for f in v["files"]][0]')" "('src/a.txt', 1, 1, ['hunk', 'del', 'add'])"
ok "  a file GitHub shows no diff for says why" "$(printf '%s' "$OUT" | val 'v["files"][1]["diff"]["empty_reason"]')" "GitHub shows no diff for this file (binary, or too large)."
ok "its commits, first line and author" "$(call github_pull_commits "$(args '"number":1')" | val '[(c["sha"], c["title"], c["author"]) for c in v]')" "[('c1', 'first', 'me-home'), ('c2', 'second', 'Someone')]"

section "Reviewing"
ok "approving your own pull request: GitHub's refusal, in words" "$(call github_pull_review '{"account":"gh:me-home","owner":"acme","name":"app","number":1,"event":"APPROVE","body":""}' | val '"does not let @me-home approve their own pull request" in v["error"]')" "True"
ok "asking for changes needs words, and nothing is sent without them" "$(N=$(wc -l < "$SB/api.log"); call github_pull_review "$(args '"number":1,"event":"REQUEST_CHANGES","body":" "')" | val 'v["error"]'; [ "$(wc -l < "$SB/api.log")" = "$N" ] || echo SENT)" "Configuration error: Write what should change first."
ok "an approval from another account goes through" "$(call github_pull_review "$(args '"number":1,"event":"APPROVE","body":"Looks good"')" | val 'v["login"], v["state"]')" "('work-me', 'APPROVED')"
ok "  with the event and the words" "$(lastreq 'r["m"], r["p"], r["b"]')" "('POST', '/repos/acme/app/pulls/1/reviews', '{\"body\":\"Looks good\",\"event\":\"APPROVE\"}')"

section "Merging, with the commit that was reviewed"
ok "someone who cannot push is refused by GitHub, in words" "$(call github_pull_merge '{"account":"gh:me-home","owner":"acme","name":"app","number":1,"method":"merge","sha":"abc1234def5678","title":null,"message":null,"deleteBranch":false}' | val '"does not let @me-home do that here" in v["error"]')" "True"
ok "a push after the review: nothing merged, and why" "$(call github_pull_merge "$(args '"number":1,"method":"squash","sha":"0000000aaaa","title":"T","message":null,"deleteBranch":true')" | val 'v["error"]')" "Command execution error: New commits were pushed to this branch after you reviewed it, so nothing was merged. Refresh, look at them, and merge again."
ok "  and the branch was not touched" "$(grep -c '"DELETE"' "$SB/api.log" || true)" "0"
OUT=$(call github_pull_merge "$(args '"number":1,"method":"squash","sha":"abc1234def5678","title":"Add the thing (#1)","message":null,"deleteBranch":true')")
ok "the reviewed commit merges" "$(printf '%s' "$OUT" | val 'v["merged"], v["sha"], v["branch_deleted"], v["branch_note"]')" "(True, 'm3rg3d', True, 'Branch feat/1 deleted.')"
ok "  GitHub got the method, the commit and the title" "$(grep '"PUT", "p": "/repos/acme/app/pulls/1/merge"' "$SB/api.log" | tail -1 | "$PY" -c 'import sys,json; print(sorted(json.loads(json.loads(sys.stdin.read())["b"]).items()))')" "[('commit_title', 'Add the thing (#1)'), ('merge_method', 'squash'), ('sha', 'abc1234def5678')]"
ok "  then the branch, and only it, was deleted" "$(grep '"DELETE"' "$SB/api.log" | "$PY" -c 'import sys,json; print([json.loads(l)["p"] for l in sys.stdin])')" "['/repos/acme/app/git/refs/heads/feat/1']"
ok "the list no longer shows it open" "$(call github_pulls "$(args '"state":"open","page":1')" | val '[i["number"] for i in v["items"]]')" "[2]"
ok "an unknown merge method is refused before GitHub is asked" "$(call github_pull_merge "$(args '"number":1,"method":"octopus","sha":"abc1234def5678","title":null,"message":null,"deleteBranch":false')" | val 'v["error"]')" "Configuration error: Unknown merge method \"octopus\""

section "Collaborators"
OUT=$(call github_collaborators "$(args)")
ok "who it is shared with, and their roles" "$(printf '%s' "$OUT" | val '[(c["login"], c["role"]) for c in v["collaborators"]]')" "[('work-me', 'admin'), ('pal', 'write')]"
ok "  the invitations still waiting (an admin sees them)" "$(printf '%s' "$OUT" | val '[(i["id"], i["login"], i["role"]) for i in v["invitations"]]')" "[(77, 'newbie', 'write')]"
ok "someone who cannot push may not list them: GitHub's refusal in words" "$(call github_collaborators '{"account":"gh:me-home","owner":"acme","name":"app"}' | val '"does not let @me-home do that here" in v["error"]')" "True"
ok "inviting a user GitHub does not know: said, nothing sent to the repository" "$(call github_collaborator_add "$(args '"username":"ghost-user","permission":"push"')" | val 'v["error"]'; grep -c '"PUT", "p": "/repos/acme/app/collaborators/ghost-user"' "$SB/api.log" || true)" "Command execution error: There is no GitHub user called ghost-user.
0"
ok "inviting someone new sends an invitation, with the role" "$(call github_collaborator_add "$(args '"username":"@friend","permission":"triage"')" | val 'v["status"], v["message"], v["invitation_id"]')" "('invited', 'Invited Friend — they get access once they accept the invitation GitHub sends them.', 78)"
ok "  as a PUT with that role" "$(lastreq 'r["m"], r["p"], r["b"]')" "('PUT', '/repos/acme/app/collaborators/friend', '{\"permission\":\"triage\"}')"
ok "inviting someone who has access already says so" "$(call github_collaborator_add "$(args '"username":"pal","permission":"push"')" | val 'v["status"]')" "added"
ok "a name that is not a GitHub username is refused before GitHub is asked" "$(call github_collaborator_add "$(args '"username":"../admin","permission":"push"')" | val 'v["error"]')" "Configuration error: \"../admin\" is not a GitHub username"
ok "an unknown role is refused" "$(call github_collaborator_add "$(args '"username":"pal","permission":"owner"')" | val 'v["error"]')" "Configuration error: Unknown role \"owner\""
call github_collaborator_remove "$(args '"username":"pal"')" >/dev/null
call github_invitation_cancel "$(args '"id":77')" >/dev/null
OUT=$(call github_collaborators "$(args)")
ok "removing someone and cancelling an invitation: both gone" "$(printf '%s' "$OUT" | val '[c["login"] for c in v["collaborators"]], [i["login"] for i in v["invitations"]]')" "(['work-me'], ['friend'])"

section "Tokens"
ok "every request carried the token of the account picked, and only known tokens were sent" "$("$PY" -c 'import sys,json; print(sorted({json.loads(l)["t"] for l in open(sys.argv[1])}))' "$SB/api.log")" "['t-me-home', 't-work-me']"
ok "  no answer ever held a token (of $(wc -l < "$SB/answers.log" | tr -d ' ') answers)" "$(grep -c 't-work-me\|t-me-home' "$SB/answers.log" || true)" "0"

verify_result
