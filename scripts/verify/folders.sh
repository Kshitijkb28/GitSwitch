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

verify_result
