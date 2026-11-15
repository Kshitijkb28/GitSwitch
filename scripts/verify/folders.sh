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

verify_result
