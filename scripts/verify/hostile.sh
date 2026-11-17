#!/bin/bash
# Hostile repositories, end to end against the real backend in a sandbox.
#
# A git command the app starts ON ITS OWN (a page opening, window focus, a
# scan, a repository nobody picked, the push card's credential lookup, the
# History fetch) never runs a program that a repository's own config,
# attributes or hooks name — an extracted archive carries its .git/config and
# .git/hooks with it. The person's own settings and explicit actions keep
# working as in a terminal. Built from the audit's two labs
# (audit/scripts/poc-hostile-repo.sh, poc-top-findings.sh).
#
#   1. git's own behaviour on this machine: each marker fires under plain git
#      (or the rows below would prove nothing), and each value the app pins
#      switches its key off;
#   2. the passive entry points: nothing the repository names runs, and every
#      git command they start is of the passive class;
#   3. the History fetch: refused over a transport program the repository
#      names, and a plain fetch otherwise;
#   4. legitimate settings still work: the person's own credential helper,
#      the profile's ssh command, a git-crypt style clean filter on commit, a
#      husky style hooks folder, submodule download with `update = !command`;
#   6. repository text never becomes a git option or a path outside the
#      repository: a crafted saved state, a branch named `--output=…`, 0x1F in
#      a commit subject, a submodule named `../../..`, a `.git/modules` that
#      is a link out of the repository. (Section 2 has a submodule at the
#      path `--cached`.)
#
# Markers are scripts that only append a line to a file in the sandbox. No
# network: remotes are local folders or example.invalid, and the ssh used is a
# fake named by absolute path that never calls the real one.
set -u
source "$(dirname "$0")/lib.sh"
verify_init hostile "Hostile Lab" "lab@example.test"          # HOME redirected FIRST
SB="$(realdir "$SB")"
# An askpass inherited from the shell running the suite (an editor's terminal
# sets one) would answer instead of the repository's in the controls below.
unset GIT_ASKPASS SSH_ASKPASS
export GIT_TERMINAL_PROMPT=0
MARK="$SB/MARK"
export PROBE_CLASS_FILE="$SB/classes.json"

mk() {  # mk <name> [filter|sed-expr] -> path of a program that notes it ran, as <name>
  local p="$SB/prog-$1.sh"
  case "${2:-}" in
    filter) printf '#!/bin/sh\necho "%s $*" >> "%s"\ncat\n' "$1" "$MARK" > "$p" ;;
    "") printf '#!/bin/sh\necho "%s $*" >> "%s"\nexit 1\n' "$1" "$MARK" > "$p" ;;
    *) printf '#!/bin/sh\necho "%s $*" >> "%s"\nsed %s\n' "$1" "$MARK" "$2" > "$p" ;;
  esac
  chmod +x "$p"; printf '%s' "$p"
}
fired() { if [ -f "$MARK" ]; then cut -d' ' -f1 "$MARK" | sort -u | tr '\n' ' ' | sed 's/ $//'; fi; }
reset() { rm -f "$MARK"; }
jstr() { "$PY" -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }
dirty() { sleep 1.1; cp "$1" "$1.tmp" && mv "$1.tmp" "$1"; }   # same bytes, a new mtime: git must read it again
newrepo() { git init -q "$1"; git -C "$1" -c user.name=x -c user.email=x@x.test commit -q --allow-empty -m first; }
profile() { "$PY" -c 'import json,sys; print(json.dumps({"name":sys.argv[1],"git_name":"Lab","git_email":"lab@example.test","directories":[sys.argv[2]]}))' "$1" "$2" > "$SB/$1.json"; probe profile_apply "$SB" "$SB/$1.json" >/dev/null 2>&1; }
# pcall <command> <json>: the real backend; what it ran is in $PROBE_CLASS_FILE
pcall() { rm -f "$PROBE_CLASS_FILE"; pinvoke "$1" "$2"; }
# classes <folder> -> "passive=<n> writes=[<the write-class commands run in the folder>]"
classes() {
  "$PY" - "$PROBE_CLASS_FILE" "$1" <<'EOF'
import json, sys
try:
    ran = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    ran = []
norm = lambda p: (p or "").replace("\\", "/").rstrip("/").lower()
root = norm(sys.argv[2])
mine = [r for r in ran if norm(r.get("repo")) == root or norm(r.get("repo")).startswith(root + "/")]
# The Workspaces page files a repository under the app's own keys
# (`git config --local gitswitch.workspace …`) when it is shown: a write, but
# `git config` runs no program whatever the repository's config says.
own = lambda a: len(a) > 0 and a[0] == "config" and any(x.startswith("gitswitch.") for x in a[1:])
writes = [" ".join(r["args"]) for r in mine if r["class"] == "write" and not own(r["args"])]
kept = sum(1 for r in mine if r["class"] == "write" and own(r["args"]))
print("passive=%d writes=[%s] own-config-writes=%d" % (sum(1 for r in mine if r["class"] == "passive"), "; ".join(writes), kept))
EOF
}
# passive_row <label> <command> <json> <folder> [idle-ok]: nothing ran, and
# only passive commands did (and some did, unless idle-ok: without git-lfs the
# Large files card runs nothing)
passive_row() {
  reset; local out; out="$(pcall "$2" "$3")"
  ok "$1: nothing the repository names ran" "ran:[$(fired)]" "ran:[]"
  local c; c="$(classes "$4")"
  ok "$1: every git it started is passive" "$c" "writes=[]"
  case "$c:${5:-}" in "passive=0 "*:) ok "$1: it started git at all" "$c" "passive=<at least 1>" ;; esac
  [ -n "$out" ] || ok "$1: the command answered" "(no reply)" "a reply"
}

echo "git: $(git --version)"

# =============================================================================
section "1. git's own behaviour here: every marker fires, every pinned value switches its key off"
G="$SB/proof"; git init -q "$G"
git -C "$G" config user.name Lab; git -C "$G" config user.email lab@example.test; git -C "$G" config commit.gpgsign false
printf '*.dat filter=evil\n*.conv diff=evil\n' > "$G/.gitattributes"
git -C "$G" config filter.evil.clean "$(mk clean "s/^/X/")"
git -C "$G" config filter.evil.required true
printf 'aaaa\n' > "$G/a.dat"; printf 'c\n' > "$G/c.conv"
git -C "$G" add -A; git -C "$G" commit -qm one
git -C "$G" config filter.evil.smudge "$(mk smudge "s/^X//")"
git -C "$G" config core.fsmonitor "$(mk fsmonitor)"
git -C "$G" config diff.evil.command "$(mk diffcmd)"
git -C "$G" config diff.evil.textconv "$(mk textconv)"
printf 'c\nd\n' > "$G/c.conv"
dirty "$G/a.dat"
export GIT_OPTIONAL_LOCKS=0      # the controls must not refresh the index behind the next row's back
reset; git -C "$G" status --porcelain >/dev/null 2>&1
ok "control: plain status runs the repository's fsmonitor and clean filter" "$(fired)" "clean fsmonitor"
reset; git -C "$G" -c core.fsmonitor= status --porcelain >/dev/null 2>&1
ok "core.fsmonitor= (empty): no fsmonitor" "ran:[$(fired)]" "ran:[clean]"
reset; out="$(git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.required=false status --porcelain 2>&1)"; rc=$?
ok "filter.<name>.clean= with required=false: no filter, status still answers" "ran:[$(fired)] rc=$rc" "ran:[] rc=0"
reset; out="$(git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= status --porcelain 2>&1)"
ok "  and why required=false: an emptied required filter is fatal" "$out" "clean filter 'evil' failed"
git -C "$G" config filter.evil.process "$(mk process)"
dirty "$G/a.dat"
reset; git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.required=false status --porcelain >/dev/null 2>&1
ok "control: a process filter runs although clean and smudge are emptied" "$(fired)" "process"
reset; git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.process= -c filter.evil.required=false status --porcelain >/dev/null 2>&1
ok "filter.<name>.process= (empty): no process filter" "ran:[$(fired)]" "ran:[]"
git -C "$G" config --unset filter.evil.process
rm "$G/a.dat"
reset; git -C "$G" -c core.fsmonitor= checkout -- a.dat >/dev/null 2>&1
ok "control: checkout runs the smudge filter" "$(fired)" "smudge"
rm "$G/a.dat"
reset; git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.required=false checkout -- a.dat >/dev/null 2>&1
ok "filter.<name>.smudge= (empty): no smudge filter" "ran:[$(fired)]" "ran:[]"
reset; git -C "$G" -c core.fsmonitor= diff -- c.conv >/dev/null 2>&1
ok "control: diff runs the diff driver's command" "$(fired)" "diffcmd"
reset; git -C "$G" -c core.fsmonitor= -c diff.evil.command= diff --no-ext-diff -- c.conv >/dev/null 2>&1
ok "control: textconv runs when only the command is off" "$(fired)" "textconv"
reset; out="$(git -C "$G" -c core.fsmonitor= -c diff.evil.command= -c diff.evil.textconv= diff -- c.conv 2>&1)"
ok "diff.<name>.command= / .textconv= (empty): nothing runs" "ran:[$(fired)]" "ran:[]"
ok "  but empty is not off: git runs an empty program name and the diff fails (the flags are what keep a diff from the drivers)" "$out" "external diff died"
reset; out="$(git -C "$G" -c core.fsmonitor= diff --no-ext-diff --no-textconv -- c.conv 2>&1)"; rc=$?
ok "--no-ext-diff --no-textconv: no driver, and a real diff" "ran:[$(fired)] rc=$rc" "ran:[] rc=0"
ok "  (the diff itself)" "$out" "+d"
# (`whatchanged` takes them too, but git 2.51+ wants `--i-still-use-this`
# before it runs at all; the app never uses it.)
for sub in "show --no-ext-diff --no-textconv HEAD" "log --no-ext-diff --no-textconv -p -1" "diff-tree --no-ext-diff --no-textconv -p HEAD" "diff-files --no-ext-diff --no-textconv -p" "diff-index --no-ext-diff --no-textconv -p HEAD"; do
  reset; out="$(git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.required=false $sub 2>&1)"; rc=$?
  ok "git $sub: accepted, nothing ran" "ran:[$(fired)] rc=$rc" "ran:[] rc=0"
done
git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.required=false stash -q
reset; out="$(git -C "$G" -c core.fsmonitor= stash show --no-ext-diff --no-textconv -p 2>&1)"; rc=$?
ok "git stash show --no-ext-diff --no-textconv: accepted, nothing ran" "ran:[$(fired)] rc=$rc" "ran:[] rc=0"
git -C "$G" -c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.required=false stash pop -q

# A commit that claims a signature of each kind (headers only, nothing is signed).
mksig() {
  git -C "$G" cat-file commit HEAD > "$SB/c.txt"
  { sed -n '1,/^$/p' "$SB/c.txt" | sed '$d'; printf 'gpgsig -----BEGIN %s-----\n %s\n -----END %s-----\n\n' "$1" "$2" "$1"; sed '1,/^$/d' "$SB/c.txt"; } | git -C "$G" hash-object -t commit -w --stdin
}
OPGP=$(mksig "PGP SIGNATURE" "iQEzBAABCAAdFiEE"); OSSH=$(mksig "SSH SIGNATURE" "U1NIU0lHAAAAAQ=="); OX509=$(mksig "SIGNED MESSAGE" "MIAGCSqGSIb3DQEHAqCAMIACAQEx")
git -C "$G" config gpg.openpgp.program "$(mk gpg)"
git -C "$G" config gpg.ssh.program "$(mk gpgssh)"
git -C "$G" config gpg.x509.program "$(mk gpgx509)"
# git checks an ssh signature only with an allowed-signers file to check it against.
printf 'lab@example.test ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEhostile\n' > "$SB/allowed-signers"
git -C "$G" config gpg.ssh.allowedSignersFile "$SB/allowed-signers"
for o in "$OPGP:gpg" "$OSSH:gpgssh" "$OX509:gpgx509"; do
  reset; git -C "$G" -c core.fsmonitor= log -1 --format=%G? "${o%%:*}" >/dev/null 2>&1
  ok "control: %G? runs the repository's ${o##*:}" "$(fired)" "${o##*:}"
  reset; git -C "$G" -c core.fsmonitor= -c gpg.program=gpg -c gpg.openpgp.program=gpg -c gpg.ssh.program=ssh-keygen -c gpg.x509.program=gpgsm log -1 --format=%G? "${o%%:*}" >/dev/null 2>&1
  ok "gpg programs set to git's default names: no ${o##*:}" "ran:[$(fired)]" "ran:[]"
done
reset; git -C "$G" -c core.fsmonitor= -c gpg.program=gpg log -1 --format=%G? "$OPGP" >/dev/null 2>&1
ok "gpg.program overrides the repository's gpg.openpgp.program (one program, the last one read)" "ran:[$(fired)]" "ran:[]"
reset; out="$(git -C "$G" -c core.fsmonitor= -c gpg.openpgp.program= log -1 --format=%G? "$OPGP" 2>&1)"
ok "  and why not empty: git runs an empty program name" "ran:[$(fired)] $out" ": No such file or directory"
git -C "$G" config format.pretty 'format:%H %G?'
reset; git -C "$G" -c core.fsmonitor= log -1 "$OPGP" >/dev/null 2>&1
ok "control: a log with no format of its own prints the repository's format.pretty, %G? and all" "$(fired)" "gpg"
reset; git -C "$G" -c core.fsmonitor= -c gpg.program=gpg -c gpg.openpgp.program=gpg log -1 "$OPGP" >/dev/null 2>&1
ok "  so such a log reads the config too, and the gpg pins switch it off" "ran:[$(fired)]" "ran:[]"
git -C "$G" config --unset format.pretty
git -C "$G" config gpg.ssh.defaultKeyCommand "$(mk defaultkey)"
printf 'n\n' > "$G/n.txt"; git -C "$G" add n.txt
reset; git -C "$G" -c core.fsmonitor= -c gpg.format=ssh -c gpg.ssh.program=ssh-keygen commit -S -qm signed >/dev/null 2>&1
ok "control: signing over ssh runs the repository's gpg.ssh.defaultKeyCommand" "$(fired)" "defaultkey"
reset; git -C "$G" -c core.fsmonitor= -c gpg.format=ssh -c gpg.ssh.program=ssh-keygen -c gpg.ssh.defaultKeyCommand= commit -S -qm signed >/dev/null 2>&1
ok "gpg.ssh.defaultKeyCommand= (empty): nothing runs (git tries an empty name and gives up)" "ran:[$(fired)]" "ran:[]"
git -C "$G" reset -q

IN='protocol=https\nhost=example.invalid\npath=a/b.git\n'
git -C "$G" config credential.helper "!$(mk localhelper)"
reset; printf "$IN" | git -C "$G" -c core.fsmonitor= credential fill >/dev/null 2>&1
ok "control: credential fill runs the repository's helper" "$(fired)" "localhelper"
reset; printf "$IN" | git -C "$G" -c core.fsmonitor= -c credential.helper= credential fill >/dev/null 2>&1
ok "credential.helper= (empty) clears the list: no helper" "ran:[$(fired)]" "ran:[]"
GH="$(mk globalhelper)"
reset; printf "$IN" | git -C "$G" -c core.fsmonitor= -c credential.helper= -c "credential.https://example.invalid.helper=!$GH" credential fill >/dev/null 2>&1
ok "  and a helper put back after it is asked" "ran:[$(fired)]" "ran:[globalhelper]"
git -C "$G" config --unset credential.helper
git -C "$G" config core.askPass "$(mk askpass)"
reset; printf "$IN" | git -C "$G" -c core.fsmonitor= credential fill >/dev/null 2>&1
ok "control: credential fill runs the repository's core.askPass (terminal prompts off)" "$(fired)" "askpass"
reset; printf "$IN" | git -C "$G" -c core.fsmonitor= -c core.askPass= credential fill >/dev/null 2>&1
ok "core.askPass= (empty): no askpass" "ran:[$(fired)]" "ran:[]"
git -C "$G" config --unset core.askPass

EMPTY="$SB/empty-hooks"; mkdir -p "$EMPTY"
printf '#!/bin/sh\necho "hook-post-index-change" >> "%s"\n' "$MARK" > "$G/.git/hooks/post-index-change"; chmod +x "$G/.git/hooks/post-index-change"
printf '#!/bin/sh\necho "hook-reference-transaction" >> "%s"\n' "$MARK" > "$G/.git/hooks/reference-transaction"; chmod +x "$G/.git/hooks/reference-transaction"
N="-c core.fsmonitor= -c filter.evil.clean= -c filter.evil.smudge= -c filter.evil.required=false"
dirty "$G/.gitattributes"
reset; git -C "$G" $N diff --quiet >/dev/null 2>&1
ok "control: diff writes the index and runs post-index-change, GIT_OPTIONAL_LOCKS=0 or not" "$(fired)" "hook-post-index-change"
dirty "$G/.gitattributes"
reset; git -C "$G" $N -c core.hooksPath="$EMPTY" diff --quiet >/dev/null 2>&1
ok "core.hooksPath=<an empty folder>: no hook" "ran:[$(fired)]" "ran:[]"
reset; git -C "$G" -c core.fsmonitor= update-ref refs/heads/probe HEAD >/dev/null 2>&1
ok "control: a ref update runs reference-transaction" "$(fired)" "hook-reference-transaction"
reset; git -C "$G" -c core.fsmonitor= -c core.hooksPath="$EMPTY" update-ref -d refs/heads/probe >/dev/null 2>&1
ok "core.hooksPath=<an empty folder>: no reference-transaction" "ran:[$(fired)]" "ran:[]"
# Without a data folder the app names /dev/null on macOS and Linux (never a
# shared folder someone else could fill): git finds no hook below it.
if ! command -v cygpath >/dev/null 2>&1; then
  dirty "$G/.gitattributes"
  reset; git -C "$G" $N -c core.hooksPath=/dev/null diff --quiet >/dev/null 2>&1
  ok "core.hooksPath=/dev/null: no post-index-change" "ran:[$(fired)]" "ran:[]"
  reset; git -C "$G" -c core.fsmonitor= -c core.hooksPath=/dev/null update-ref refs/heads/probe HEAD >/dev/null 2>&1; rc=$?
  ok "core.hooksPath=/dev/null: no reference-transaction, and the ref is written" "ran:[$(fired)] rc=$rc $(git -C "$G" rev-parse -q --verify refs/heads/probe >/dev/null && echo written)" "ran:[] rc=0 written"
  git -C "$G" -c core.hooksPath="$EMPTY" update-ref -d refs/heads/probe
fi
ok "rev-parse --git-path hooks follows the pin (why rev-parse never gets it)" "$(git -C "$G" -c core.hooksPath="$EMPTY" rev-parse --git-path hooks)" "empty-hooks"
rm -f "$G/.git/hooks/post-index-change" "$G/.git/hooks/reference-transaction"

git -C "$G" config protocol.ext.allow always
git -C "$G" remote add ext "ext::$(mk ext)"
reset; git -C "$G" -c core.fsmonitor= fetch ext </dev/null >/dev/null 2>&1
ok "control: an ext:: remote runs its command when the repository allows ext" "$(fired)" "ext"
reset; out="$(git -C "$G" -c core.fsmonitor= -c protocol.ext.allow=never fetch ext </dev/null 2>&1)"
ok "protocol.ext.allow=never: refused" "ran:[$(fired)] $out" "transport 'ext' not allowed"
git -C "$G" remote remove ext; git -C "$G" config --unset protocol.ext.allow
unset GIT_OPTIONAL_LOCKS

# The submodule rows: a superproject whose submodule is not downloaded yet.
S0="$SB/subsrc"; newrepo "$S0"; printf 's\n' > "$S0/s.txt"; git -C "$S0" add s.txt; git -C "$S0" -c user.name=x -c user.email=x@x.test commit -qm s
S="$SB/super"; newrepo "$S"
git -C "$S" submodule add -q "$S0" sub >/dev/null 2>&1; git -C "$S" -c user.name=x -c user.email=x@x.test commit -qm sub
git -C "$S" submodule deinit -q -f sub >/dev/null 2>&1
git -C "$S" config submodule.sub.update "!$(mk updatecmd)"
reset; git -C "$S" submodule update --init -- sub >/dev/null 2>&1
ok "control: a first download runs the repository's submodule.<name>.update = !command" "$(fired)" "updatecmd"
git -C "$S" submodule deinit -q -f sub >/dev/null 2>&1; git -C "$S" config submodule.sub.update "!$(mk updatecmd)"
reset; git -C "$S" submodule update --init --checkout -- sub >/dev/null 2>&1
ok "--checkout: the recorded commit is checked out and the command never runs" "ran:[$(fired)] $(git -C "$S/sub" rev-parse HEAD 2>&1)" "ran:[] $(git -C "$S0" rev-parse HEAD)"
git -C "$S" submodule deinit -q -f sub >/dev/null 2>&1; git -C "$S" config submodule.sub.update "!$(mk updatecmd)"

# =============================================================================
section "2. Passive entry points through the real backend: nothing the repository names runs"
H="$SB/hostile"; git init -q "$H"
git -C "$H" config user.name Lab; git -C "$H" config user.email lab@example.test; git -C "$H" config commit.gpgsign false
printf 'one\n' > "$H/a.txt"; printf 'data\n' > "$H/b.dat"; printf 'x\n' > "$H/c.conv"
printf '*.dat filter=evil\n*.conv diff=evil\n' > "$H/.gitattributes"
git -C "$H" config filter.evil.clean "$(mk filterclean "s/^/X/")"
git -C "$H" add -A; git -C "$H" commit -qm first
G0="$G"; G="$H"; HASH=$(mksig "PGP SIGNATURE" "iQEzBAABCAAdFiEE"); G="$G0"
git -C "$H" update-ref refs/heads/main "$HASH"
printf 'two\n' >> "$H/a.txt"; printf 'y\n' >> "$H/c.conv"; dirty "$H/b.dat"
git -C "$H" remote add origin https://example.invalid/acme/api.git
git -C "$H" config core.fsmonitor "$(mk fsmonitor)"
git -C "$H" config filter.evil.smudge "$(mk filtersmudge filter)"
git -C "$H" config filter.evil.required true
git -C "$H" config gpg.program "$(mk gpg)"
git -C "$H" config gpg.ssh.program "$(mk gpgssh)"
git -C "$H" config diff.external "$(mk diffexternal)"
git -C "$H" config diff.evil.textconv "$(mk textconv)"
git -C "$H" config diff.evil.command "$(mk diffcmd)"
git -C "$H" config core.pager "$(mk pager)"
git -C "$H" config core.editor "$(mk editor)"
git -C "$H" config core.sshCommand "$(mk sshcommand)"
git -C "$H" config credential.helper "!$(mk credhelper)"
git -C "$H" config core.askPass "$(mk askpass)"
for h in post-index-change post-checkout reference-transaction pre-auto-gc; do
  printf '#!/bin/sh\necho "hook-%s $*" >> "%s"\nexit 0\n' "$h" "$MARK" > "$H/.git/hooks/$h"; chmod +x "$H/.git/hooks/$h"
done
P="$(jstr "$H")"
passive_row "Changes tab status (changes_repo_status)"            changes_repo_status      "{\"repoPath\":$P}" "$H"
passive_row "Changes diff of a changed file"                     changes_file_diff        "{\"repoPath\":$P,\"path\":\"a.txt\",\"staged\":false,\"untracked\":false}" "$H"
passive_row "Changes diff, textconv attribute"                   changes_file_diff        "{\"repoPath\":$P,\"path\":\"c.conv\",\"staged\":false,\"untracked\":false}" "$H"
passive_row "Changes diff, filter attribute"                     changes_file_diff        "{\"repoPath\":$P,\"path\":\"b.dat\",\"staged\":false,\"untracked\":false}" "$H"
passive_row "History head"                                       history_head             "{\"repoPath\":$P}" "$H"
passive_row "History page"                                       history_page             "{\"repoPath\":$P,\"rev\":\"main\",\"offset\":0,\"limit\":50}" "$H"
passive_row "History branches"                                   history_branches         "{\"repoPath\":$P}" "$H"
passive_row "History people"                                     history_authors          "{\"repoPath\":$P,\"rev\":\"main\"}" "$H"
passive_row "Commit detail (the signature format)"               history_commit_detail    "{\"repoPath\":$P,\"hash\":\"$HASH\"}" "$H"
passive_row "File diff in a commit"                              history_commit_file_diff "{\"repoPath\":$P,\"hash\":\"$HASH\",\"path\":\"a.txt\"}" "$H"
passive_row "History sync status"                                history_sync_status      "{\"repoPath\":$P,\"branch\":\"main\"}" "$H"
passive_row "Submodules card"                                    changes_submodules       "{\"repoPath\":$P}" "$H"
passive_row "Push state"                                         changes_push_state       "{\"repoPath\":$P}" "$H"
passive_row "Push card's credential lookup (https remote)"       changes_push_identity    "{\"repoPath\":$P,\"refresh\":false}" "$H"
passive_row "Large files card"                                   changes_lfs_status       "{\"repoPath\":$P}" "$H" idle-ok
passive_row "Stashes card"                                       changes_stash_list       "{\"repoPath\":$P}" "$H"
passive_row "States card"                                        states_list              "{\"repoPath\":$P}" "$H"
passive_row "Workspaces (workspace_info)"                        workspace_info           "{\"path\":$P}" "$H"
passive_row "Folder state"                                       folder_state             "{\"path\":$P}" "$H" idle-ok
passive_row "Auto Assign scan of the parent folder"              scan_repos               "{\"root\":$(jstr "$SB")}" "$SB" idle-ok
passive_row "Find repositories over the parent folder"           find_unlisted_repos      "{\"scanId\":\"hostile\",\"extraRoots\":[$(jstr "$SB")]}" "$SB"
# The marker of the credential lookup row is only meaningful if the lookup
# reached `credential fill`: it must have, over https.
ok "  (the lookup reached git's credential protocol)" "$(pcall changes_push_identity "{\"repoPath\":$P,\"refresh\":false}")" "https-credential"

# Commit Audit: a planted repository under a profile folder, with a commit
# whose author is not the profile's (lab 2, GIT-B-01).
P1="$SB/prof1"; mkdir -p "$P1"; newrepo "$P1/planted"
git -C "$P1/planted" config core.fsmonitor "$(mk fsmonitor-audit)"
printf '#!/bin/sh\necho "hook-audit-post-index-change" >> "%s"\n' "$MARK" > "$P1/planted/.git/hooks/post-index-change"; chmod +x "$P1/planted/.git/hooks/post-index-change"
profile LabOne "$P1"
passive_row "Commit Audit over a profile folder (audit_commits)" audit_commits "{}" "$P1"
ok "  (the planted repository was audited)" "$(pcall audit_commits '{}')" "planted"

# Workspaces: viewing a clean checkout reads every other checkout that claims
# the same origin (lab 2, GIT-B-02).
W="$SB/work"; mkdir -p "$W"
newrepo "$W/real";    git -C "$W/real"    remote add origin https://github.com/example/demo.git
newrepo "$W/planted"; git -C "$W/planted" remote add origin git@github.com:Example/Demo.git
git -C "$W/planted" config core.fsmonitor "$(mk fsmonitor-sibling)"
profile LabTwo "$W"
passive_row "Workspaces: the clean checkout, a planted sibling" workspace_info "{\"path\":$(jstr "$W/real")}" "$W"

# Submodules: a status or a diff against the work tree in a superproject makes
# git run `git status` inside each downloaded submodule, which reads THAT
# repository's config. Here a submodule, the submodule nested in it and a
# gitlink no .gitmodules maps each name a clean filter of their own.
srcrepo() {  # srcrepo <folder> <filter name>
  newrepo "$1"; printf 'data\n' > "$1/f.dat"; printf '*.dat filter=%s\n' "$2" > "$1/.gitattributes"
  git -C "$1" add -A; git -C "$1" -c user.name=x -c user.email=x@x.test commit -qm "$2"
}
srcrepo "$SB/nest-src" nestevil
srcrepo "$SB/sub-src" subevil
git -C "$SB/sub-src" submodule add -q "$SB/nest-src" nest >/dev/null 2>&1; git -C "$SB/sub-src" -c user.name=x -c user.email=x@x.test commit -qm nest
T="$SB/withsubs"; newrepo "$T"; printf 'a\n' > "$T/a.txt"; git -C "$T" add a.txt; git -C "$T" -c user.name=x -c user.email=x@x.test commit -qm a
git -C "$T" submodule add -q "$SB/sub-src" sub >/dev/null 2>&1; git -C "$T" -c user.name=x -c user.email=x@x.test commit -qm sub
git -C "$T" submodule update --init --recursive -q >/dev/null 2>&1
git clone -q "$SB/sub-src" "$T/planted"
git -C "$T" update-index --add --cacheinfo "160000,$(git -C "$T/planted" rev-parse HEAD),planted"
git -C "$T/sub" config filter.subevil.clean "$(mk subclean filter)"
git -C "$T/sub/nest" config filter.nestevil.clean "$(mk nestclean filter)"
git -C "$T/planted" config filter.subevil.clean "$(mk plantedclean filter)"
git -C "$T/sub" config core.fsmonitor "$(mk subfsmonitor)"
printf 'b\n' >> "$T/a.txt"; printf 'u\n' > "$T/sub/new.txt"
# The same bytes again, so git must read each file through its filter (a size
# that changed would say "changed" without reading it).
subdirty() { sleep 1.1; for f in "$T/sub/f.dat" "$T/sub/nest/f.dat" "$T/planted/f.dat"; do cp "$f" "$f.tmp" && mv "$f.tmp" "$f"; done; }
PT="$(jstr "$T")"
subdirty; passive_row "Submodules: the superproject's Changes status"   changes_repo_status        "{\"repoPath\":$PT}" "$T"
subdirty; passive_row "Submodules: the Changes diff of a file"          changes_file_diff          "{\"repoPath\":$PT,\"path\":\"a.txt\",\"staged\":false,\"untracked\":false}" "$T"
subdirty; passive_row "Submodules: the Changes diff of the submodule"   changes_file_diff          "{\"repoPath\":$PT,\"path\":\"sub\",\"staged\":false,\"untracked\":false}" "$T"
subdirty; passive_row "Submodules: each submodule's status"             changes_submodule_statuses "{\"repoPath\":$PT}" "$T"
subdirty; passive_row "Submodules: the Submodules card"                 changes_submodules         "{\"repoPath\":$PT}" "$T"
subdirty; passive_row "Submodules: the History page"                    history_page               "{\"repoPath\":$PT,\"rev\":\"HEAD\",\"offset\":0,\"limit\":50}" "$T"
ok "  (the Changes status still sees inside the submodule: its untracked file)" "$(pcall changes_repo_status "{\"repoPath\":$PT}")" "\"sub_untracked\":true"
subdirty; reset; GIT_OPTIONAL_LOCKS=0 git -C "$T" status --porcelain >/dev/null 2>&1
for m in subclean nestclean plantedclean subfsmonitor; do
  ok "control: plain status in the superproject runs the submodules' own $m" "$(fired)" "$m"
done
# A submodule at the path `--cached` (or `--staged`, `--no-index`,
# `--ignore-submodules`): read as an option, the path would have switched off
# the check of the submodules for the diff of that very row.
git clone -q "$SB/sub-src" "$T/--cached"
git -C "$T" update-index --add --cacheinfo "160000,$(git -C "$T/--cached" rev-parse HEAD),--cached"
git -C "$T/--cached" config filter.subevil.clean "$(mk dashclean filter)"
dashdirty() { sleep 1.1; cp "$T/--cached/f.dat" "$T/--cached/f.dat.tmp" && mv "$T/--cached/f.dat.tmp" "$T/--cached/f.dat"; }
dashdirty; passive_row "Submodules: the Changes diff of a submodule at the path --cached" changes_file_diff "{\"repoPath\":$PT,\"path\":\"--cached\",\"staged\":false,\"untracked\":false}" "$T"
ok "  (the diff answered)" "$(pcall changes_file_diff "{\"repoPath\":$PT,\"path\":\"--cached\",\"staged\":false,\"untracked\":false}" | jqf ok)" "True"
dashdirty; reset; GIT_OPTIONAL_LOCKS=0 git -C "$T" diff --ignore-submodules=none -- --cached >/dev/null 2>&1
ok "control: plain git diff --ignore-submodules=none -- --cached runs that submodule's own clean filter" "$(fired)" "dashclean"
# The Changes diff names its path as `:(literal)<path>`: a file named `x[y]`
# is a pattern to git that matches `xy` too, and an inherited
# GIT_LITERAL_PATHSPECS=1 would read the magic as part of the name (an empty
# diff). Each row fails with its half of the fix taken out.
G="$SB/globname"; newrepo "$G"
printf 'one\n' > "$G/x[y]"; printf 'one\n' > "$G/xy"; printf 'one\n' > "$G/a.txt"
git -C "$G" add -A; git -C "$G" -c user.name=x -c user.email=x@x.test commit -qm files
printf 'bracket-change\n' >> "$G/x[y]"; printf 'plain-change\n' >> "$G/xy"; printf 'a-change\n' >> "$G/a.txt"
PG="$(jstr "$G")"
out="$(pcall changes_file_diff "{\"repoPath\":$PG,\"path\":\"x[y]\",\"staged\":false,\"untracked\":false}")"
ok "a file named x[y]: its Changes diff shows that file" "$out" "bracket-change"
ok "  and not xy, which the name matches as a pattern" "$(printf '%s' "$out" | grep -q plain-change && echo SHOWN || echo absent)" "absent"
ok "control: plain git diff -- 'x[y]' shows xy too" "$(git -C "$G" diff -- 'x[y]')" "plain-change"
out="$(GIT_LITERAL_PATHSPECS=1 pcall changes_file_diff "{\"repoPath\":$PG,\"path\":\"a.txt\",\"staged\":false,\"untracked\":false}")"
ok "GIT_LITERAL_PATHSPECS=1 inherited: the Changes diff of a changed file still shows the change" "$out" "a-change"
ok "control: GIT_LITERAL_PATHSPECS=1 git diff -- ':(literal)a.txt' is empty" "$([ -z "$(GIT_LITERAL_PATHSPECS=1 git -C "$G" diff -- ':(literal)a.txt')" ] && echo empty || echo shown)" "empty"
# L2: the switch is removed from every command, not only the Changes diff.
GFILES="$(git -C "$G" rev-parse HEAD)"
ok "GIT_LITERAL_PATHSPECS=1 inherited: History's diff of a file in a commit shows it" "$(GIT_LITERAL_PATHSPECS=1 pcall history_commit_file_diff "{\"repoPath\":$PG,\"hash\":\"$GFILES\",\"path\":\"a.txt\"}")" "+one"
GIT_LITERAL_PATHSPECS=1 pcall changes_stage "{\"repoPath\":$PG,\"paths\":[\"x[y]\"]}" >/dev/null
ok "  and Stage of x[y] stages that file, not xy" "$(git -C "$G" diff --cached --name-only | tr '\n' ' ')" "x[y] "
git -C "$G" reset -q
printf 'u\n' > "$G/new.txt"; git -C "$G" stash -u -q
ok "  and a stashed new file's diff shows it" "$(GIT_LITERAL_PATHSPECS=1 pcall changes_stash_file_diff "{\"repoPath\":$PG,\"index\":0,\"path\":\"new.txt\"}")" "+u"
ok "control: with GIT_ICASE_PATHSPECS=1, git --literal-pathspecs dies" "$(GIT_ICASE_PATHSPECS=1 git -C "$G" --literal-pathspecs ls-files -- a.txt 2>&1)" "incompatible"

# A submodule folder git cannot open as a repository of its own (an empty
# .git folder): git's own status says so, and the check of the submodules
# must not walk back into the superproject until it gives up first.
BK="$SB/broken"; newrepo "$BK"; printf 'a\n' > "$BK/a.txt"; git -C "$BK" add a.txt; git -C "$BK" -c user.name=x -c user.email=x@x.test commit -qm a
git -C "$BK" update-index --add --cacheinfo "160000,$(git -C "$BK" rev-parse HEAD),gone"
mkdir -p "$BK/gone/.git"
reset; out="$(pcall changes_repo_status "{\"repoPath\":$(jstr "$BK")}")"
ok "a submodule folder with an empty .git: the Changes status gets git's own answer, as in a terminal" "$out" "not recognized as a git repository"
case "$out" in *"deeper than it checks"*|*"GitSwitch did not run"*) ok "  not a refusal of the app's own" "$out" "git's own answer" ;; esac

# Filter and driver names that are not UTF-8: `-c` cannot name them again (a
# decoded name is another filter, and Windows passes arguments as UTF-16), so
# the passive command is refused instead of running them.
BX="$SB/bytes"; newrepo "$BX"
printf '* filter=\377\n' > "$BX/.gitattributes"; printf 'x\n' > "$BX/f.txt"
git -C "$BX" add -A; git -C "$BX" -c user.name=x -c user.email=x@x.test commit -qm bytes
printf '[filter "\377"]\n\tclean = %s\n' "$(mk byteclean filter)" >> "$BX/.git/config"
dirty "$BX/f.txt"; reset; out="$(pcall changes_repo_status "{\"repoPath\":$(jstr "$BX")}")"
ok "a filter whose name is not UTF-8: the Changes status is refused, saying why" "$out" "cannot be switched off for one command"
ok "  nothing ran" "ran:[$(fired)]" "ran:[]"
dirty "$BX/f.txt"; reset; GIT_OPTIONAL_LOCKS=0 git -C "$BX" status --porcelain >/dev/null 2>&1
ok "control: plain status runs that filter" "$(fired)" "byteclean"

# diff.submodule=diff in the superproject's own config makes a patch that
# touches a submodule run `git diff` inside it, with the submodule's own
# diff.external and textconv.
DS0="$SB/dsub-src"; newrepo "$DS0"; printf 'a\n' > "$DS0/a.txt"; printf 'c\n' > "$DS0/c.conv"; printf '*.conv diff=evil\n' > "$DS0/.gitattributes"
git -C "$DS0" add -A; git -C "$DS0" -c user.name=x -c user.email=x@x.test commit -qm a
DS="$SB/dsuper"; newrepo "$DS"
git -C "$DS" submodule add -q "$DS0" sub >/dev/null 2>&1; git -C "$DS" -c user.name=x -c user.email=x@x.test commit -qm sub
git -C "$DS" config diff.submodule diff
git -C "$DS/sub" config diff.external "$(mk subexternal)"
git -C "$DS/sub" config diff.evil.textconv "$(mk subtextconv)"
dsmove() { printf '%s\n' "$1" >> "$DS/sub/a.txt"; printf '%s\n' "$1" >> "$DS/sub/c.conv"; git -C "$DS/sub" -c user.name=x -c user.email=x@x.test commit -qam "$1"; }
dsmove one; git -C "$DS" add sub; git -C "$DS" -c user.name=x -c user.email=x@x.test commit -qm "sub moved"
DSC=$(git -C "$DS" rev-parse HEAD)
# git stash leaves submodules out, so the stash that moves one is built by hand.
dsmove two; git -C "$DS" update-index --cacheinfo "160000,$(git -C "$DS/sub" rev-parse HEAD),sub"
DST=$(git -C "$DS" write-tree); git -C "$DS" reset -q
DSI=$(git -C "$DS" commit-tree "$DST" -p HEAD -m "index on main")
DSW=$(git -C "$DS" commit-tree "$DST" -p HEAD -p "$DSI" -m "WIP on main: sub moved")
git -C "$DS" stash store -m "WIP on main: sub moved" "$DSW"
dsmove three; printf 'dirty\n' >> "$DS/sub/c.conv"
PD="$(jstr "$DS")"
passive_row "diff.submodule=diff: the Changes diff of the submodule"  changes_file_diff        "{\"repoPath\":$PD,\"path\":\"sub\",\"staged\":false,\"untracked\":false}" "$DS"
passive_row "diff.submodule=diff: the History file diff of the gitlink" history_commit_file_diff "{\"repoPath\":$PD,\"hash\":\"$DSC\",\"path\":\"sub\"}" "$DS"
passive_row "diff.submodule=diff: the stash file diff of the gitlink"   changes_stash_file_diff  "{\"repoPath\":$PD,\"index\":0,\"path\":\"sub\"}" "$DS"
ok "  (the History file diff is the submodule's commit line)" "$(pcall history_commit_file_diff "{\"repoPath\":$PD,\"hash\":\"$DSC\",\"path\":\"sub\"}")" "Subproject commit"
ok "  (the stash file diff is too)" "$(pcall changes_stash_file_diff "{\"repoPath\":$PD,\"index\":0,\"path\":\"sub\"}")" "Subproject commit"
reset; GIT_OPTIONAL_LOCKS=0 git -C "$DS" diff -- sub >/dev/null 2>&1
ok "control: plain git diff -- sub runs the submodule's own diff.external" "$(fired)" "subexternal"

# A partial clone (an extracted archive can be one): a passive read that
# needs a missing object makes git fetch it on its own, with the repository's
# own core.sshCommand.
PR="$SB/promsrc"; newrepo "$PR"; printf 'v1\n' > "$PR/f.txt"; git -C "$PR" add f.txt; git -C "$PR" -c user.name=x -c user.email=x@x.test commit -qm v1
printf 'v2\n' > "$PR/f.txt"; git -C "$PR" -c user.name=x -c user.email=x@x.test commit -qam v2
PC="$SB/partial"; git clone -q --no-local "$PR" "$PC"
V1=$(git -C "$PC" rev-parse HEAD~1:f.txt); V2=$(git -C "$PC" rev-parse HEAD:f.txt); PCOLD=$(git -C "$PC" rev-parse HEAD~1)
git -C "$PC" repack -adq; PK="$(ls "$PC/.git/objects/pack/"*.pack)"
git -C "$PC" rev-list --objects --all | cut -d' ' -f1 | grep -v -e "^$V1" -e "^$V2" | git -C "$PC" pack-objects -q "$PC/.git/objects/pack/kept" >/dev/null
rm -f "${PK%.pack}".*
git -C "$PC" config core.repositoryformatversion 1
git -C "$PC" config extensions.partialClone origin
git -C "$PC" config remote.origin.promisor true
git -C "$PC" config remote.origin.url ssh://example.invalid/acme/partial.git
git -C "$PC" config core.sshCommand "$(mk partialssh)"
printf 'v3\n' > "$PC/f.txt"
# (rev-list --missing=print names what is missing without fetching it, git 2.17+)
missing() { git -C "$1" rev-list --objects --missing=print --all 2>/dev/null | grep -c "^?$2"; }
ok "  (both versions of f.txt are gone from the partial clone)" "$(missing "$PC" "$V1") $(missing "$PC" "$V2")" "1 1"
PP="$(jstr "$PC")"
passive_row "Partial clone: the Changes status"              changes_repo_status      "{\"repoPath\":$PP}" "$PC"
passive_row "Partial clone: the Changes diff"                changes_file_diff        "{\"repoPath\":$PP,\"path\":\"f.txt\",\"staged\":false,\"untracked\":false}" "$PC"
passive_row "Partial clone: the History file diff"           history_commit_file_diff "{\"repoPath\":$PP,\"hash\":\"$PCOLD\",\"path\":\"f.txt\"}" "$PC"
ok "  (refused, naming the key)" "$(pcall changes_file_diff "{\"repoPath\":$PP,\"path\":\"f.txt\",\"staged\":false,\"untracked\":false}")" "partial clone whose own settings name core.sshCommand"
# The History page passes such a refusal on: an error saying why, never an
# empty history with no word.
reset; out="$(pcall history_page "{\"repoPath\":$PP,\"rev\":\"HEAD\",\"offset\":0,\"limit\":50}")"
ok "Partial clone: the History page is an error, not an empty history" "ok=$(printf '%s' "$out" | jqf ok)" "ok=False"
ok "  naming the key" "$(printf '%s' "$out" | jqf error)" "partial clone whose own settings name core.sshCommand"
ok "  nothing ran" "ran:[$(fired)]" "ran:[]"
reset; GIT_OPTIONAL_LOCKS=0 git -C "$PC" diff --numstat >/dev/null 2>&1
ok "control: plain git diff --numstat fetches the missing object with the repository's core.sshCommand" "$(fired)" "partialssh"
# The app's own partial clones (no transport program of the repository's)
# still fetch what a History diff needs, asking only the person's helpers.
git -C "$PR" config uploadpack.allowFilter true
PO="$SB/partial-own"; git clone -q --filter=blob:none "file://$PR" "$PO" 2>/dev/null
git -C "$PO" config credential.helper "!$(mk partialhelper)"
POOLD=$(git -C "$PO" rev-parse HEAD~1)
ok "  (the app's kind of partial clone lacks the old version)" "$(missing "$PO" "$V1")" "1"
reset; out="$(pcall history_commit_file_diff "{\"repoPath\":$(jstr "$PO"),\"hash\":\"$POOLD\",\"path\":\"f.txt\"}")"
ok "a partial clone without such settings: the History file diff fetches what it needs" "$out" "v1"
ok "  nothing the repository names ran" "ran:[$(fired)]" "ran:[]"

# git-lfs: the person's own filter (git lfs install, run outside any
# repository), and repositories that use it.
if git lfs version >/dev/null 2>&1; then
  ( cd "$SB" && git lfs install --skip-repo >/dev/null 2>&1 )
  # An LFS extension in the repository's own config is a program git-lfs's
  # clean runs. git-lfs starts it by its first word, found on PATH (a script
  # cannot be started by its path on Windows): sh runs the marker.
  LX="$SB/lfsext"; newrepo "$LX"
  printf '*.bin filter=lfs diff=lfs merge=lfs -text\n' > "$LX/.gitattributes"; printf 'binary\n' > "$LX/a.bin"
  git -C "$LX" add -A; git -C "$LX" -c user.name=x -c user.email=x@x.test commit -qm bin
  git -C "$LX" config lfs.extension.evil.clean "sh $(mk lfsextclean filter) %f"
  git -C "$LX" config lfs.extension.evil.smudge "sh $(mk lfsextsmudge filter) %f"
  git -C "$LX" config lfs.extension.evil.priority 0
  PL="$(jstr "$LX")"
  dirty "$LX/a.bin"; passive_row "LFS extension: the Changes status" changes_repo_status "{\"repoPath\":$PL}" "$LX"
  dirty "$LX/a.bin"; passive_row "LFS extension: the Changes diff"   changes_file_diff   "{\"repoPath\":$PL,\"path\":\"a.bin\",\"staged\":false,\"untracked\":false}" "$LX"
  dirty "$LX/a.bin"; passive_row "LFS extension: the Large files card" changes_lfs_status "{\"repoPath\":$PL}" "$LX"
  dirty "$LX/a.bin"; reset; GIT_OPTIONAL_LOCKS=0 git -C "$LX" status --porcelain >/dev/null 2>&1
  ok "control: plain status runs the repository's LFS extension through the person's git-lfs" "$(fired)" "lfsextclean"

  # The Large files card lists LFS files with `git lfs ls-files`, which diffs
  # the work tree through git: a status in every downloaded submodule, with
  # that submodule's own filters.
  srcrepo "$SB/lsub-src" lsubevil
  LQ="$SB/lfssuper"; newrepo "$LQ"
  printf '*.bin filter=lfs diff=lfs merge=lfs -text\n' > "$LQ/.gitattributes"; printf 'big\n' > "$LQ/big.bin"
  git -C "$LQ" add -A; git -C "$LQ" -c user.name=x -c user.email=x@x.test commit -qm big
  git -C "$LQ" submodule add -q "$SB/lsub-src" sub >/dev/null 2>&1; git -C "$LQ" -c user.name=x -c user.email=x@x.test commit -qm sub
  git -C "$LQ/sub" config filter.lsubevil.clean "$(mk lsubclean filter)"
  PQ="$(jstr "$LQ")"
  dirty "$LQ/sub/f.dat"; passive_row "Large files card on an LFS repository with a submodule" changes_lfs_status "{\"repoPath\":$PQ}" "$LQ"
  ok "  (the card counted the LFS file)" "$(pcall changes_lfs_status "{\"repoPath\":$PQ}")" "\"tracked\":1"
  dirty "$LQ/sub/f.dat"; reset; GIT_OPTIONAL_LOCKS=0 git -C "$LQ" lfs ls-files --json >/dev/null 2>&1
  ok "control: plain git lfs ls-files runs the submodule's own clean filter" "$(fired)" "lsubclean"
else
  echo "  (git-lfs is not installed: the LFS extension and Large files rows are skipped)"
fi

# =============================================================================
section "3. The History fetch"
reset; out="$(pcall history_fetch "{\"repoPath\":$P}")"
ok "refused over the repository's own core.sshCommand, naming it" "$out" "core.sshCommand"
ok "  and saying what to do instead" "$out" "Fetch from a terminal, or move the setting into your profile"
ok "  nothing ran" "ran:[$(fired)]" "ran:[]"
ok "  every git it started is passive" "$(classes "$H")" "writes=[]"
FR="$SB/fetchremote"; newrepo "$FR"
# Over git's transport (`--no-local`), so the clone starts as one pack.
F="$SB/fetcher"; git clone -q --no-local "$FR" "$F"
git -C "$F" config credential.helper "!$(mk fetchhelper)"
git -C "$F" config core.askPass "$(mk fetchaskpass)"
# The housekeeping after the fetch asks pre-auto-gc only once there is
# something to tidy: more packs than gc.autoPackLimit allows. The fetch keeps
# what arrives as a pack, so one fetch crosses a limit of 1; no detaching, so
# what housekeeping did is on disk when it returns. The plan is git's classic
# gc: since 2.54 git repacks geometrically by default, which never reads
# gc.autoPackLimit.
git -C "$F" config gc.autoPackLimit 1; git -C "$F" config fetch.unpackLimit 1; git -C "$F" config gc.autoDetach false; git -C "$F" config maintenance.strategy gc
for h in reference-transaction pre-auto-gc post-index-change; do
  printf '#!/bin/sh\necho "hook-fetch-%s" >> "%s"\nexit 0\n' "$h" "$MARK" > "$F/.git/hooks/$h"; chmod +x "$F/.git/hooks/$h"
done
packs() { git -C "$F" count-objects -v | grep -E '^(count|packs):' | tr '\n' ' ' | sed 's/ $//'; }
ok "(the clone is one pack)" "$(packs)" "count: 0 packs: 1"
git -C "$FR" -c user.name=x -c user.email=x@x.test commit -q --allow-empty -m second
reset; out="$(pcall history_fetch "{\"repoPath\":$(jstr "$F")}")"
ok "without such a key it fetches" "$out" "remote branch"
ok "  nothing the repository names ran (hooks, helper, askpass)" "ran:[$(fired)]" "ran:[]"
ok "  every git it started is passive" "$(classes "$F")" "writes=[]"
ok "  the remote-tracking branch moved" "$(git -C "$F" rev-parse origin/main)" "$(git -C "$FR" rev-parse HEAD)"
ok "  its housekeeping crossed gc.autoPackLimit and tidied (two packs back to one): pre-auto-gc was asked for, and not run" "$(packs)" "count: 0 packs: 1"
git -C "$FR" -c user.name=x -c user.email=x@x.test commit -q --allow-empty -m second-b
git -C "$F" -c core.hooksPath="$SB/empty-hooks" -c maintenance.auto=false -c gc.auto=0 fetch -q origin >/dev/null 2>&1
ok "(a second pack again)" "$(packs)" "count: 0 packs: 2"
reset; git -C "$F" maintenance run --auto --quiet >/dev/null 2>&1
ok "control: plain housekeeping (maintenance run --auto) runs the repository's pre-auto-gc" "$(fired)" "hook-fetch-pre-auto-gc"
git -C "$FR" -c user.name=x -c user.email=x@x.test commit -q --allow-empty -m third
reset; git -C "$F" fetch -q origin >/dev/null 2>&1
ok "control: a plain fetch runs the repository's reference-transaction hook" "$(fired)" "hook-fetch-reference-transaction"

# =============================================================================
section "4. Legitimate settings and explicit actions still work"
# The person's own (global) credential helper answers the push card, scoped to
# its URL; the repository's own helper does not.
PH="$SB/prog-personalhelper.sh"
printf '#!/bin/sh\necho "personalhelper $*" >> "%s"\necho username=globaluser\necho password=globalpw\n' "$MARK" > "$PH"; chmod +x "$PH"
git config --global "credential.https://example.invalid.helper" "!$PH"
C1="$SB/creds"; newrepo "$C1"; git -C "$C1" remote add origin https://example.invalid/acme/api.git
git -C "$C1" config credential.helper "!$(mk localhelper)"
reset; out="$(pcall changes_push_identity "{\"repoPath\":$(jstr "$C1"),\"refresh\":false}")"
ok "a global credential helper still answers credential fill" "$out" "\"account\":\"globaluser\""
ok "  the person's helper ran, the repository's did not" "$(fired)" "personalhelper"
case " $(fired) " in *" localhelper "*) ok "  no local helper" "localhelper ran" "it did not" ;; esac
C2="$SB/creds-other"; newrepo "$C2"; git -C "$C2" remote add origin https://other.invalid/acme/api.git
reset; out="$(pcall changes_push_identity "{\"repoPath\":$(jstr "$C2"),\"refresh\":false}")"
ok "  a helper scoped to another URL still does not answer" "ran:[$(fired)] $out" "ran:[]"
git config --global --unset "credential.https://example.invalid.helper"

# The profile's ssh command (global scope, through includeIf) is the one an
# explicit fetch uses, and the History fetch too. A fake ssh, by absolute
# path, notes that it ran and never calls the real one.
FAKE_SSH="$(mk fakessh)"
mkdir -p "$SB/profdir"
printf '[core]\n\tsshCommand = %s\n' "$FAKE_SSH" > "$SB/profile-ssh.inc"
printf '[includeIf "gitdir:%s/"]\n\tpath = %s\n' "$SB/profdir" "$SB/profile-ssh.inc" >> "$HOME/.gitconfig"
K="$SB/profdir/app"; newrepo "$K"; git -C "$K" remote add origin git@example.invalid:acme/app.git
git -C "$K" config branch.main.remote origin; git -C "$K" config branch.main.merge refs/heads/main
reset; out="$(pcall changes_pull "{\"repoPath\":$(jstr "$K"),\"mode\":\"ff-only\",\"withLfs\":false,\"autostash\":false}")"
ok "an explicit fetch (Pull) uses the profile's includeIf core.sshCommand" "$(fired)" "fakessh"
reset; out="$(pcall history_fetch "{\"repoPath\":$(jstr "$K")}")"
ok "the History fetch uses it too (the person's own scope)" "$(fired)" "fakessh"

# A git-crypt style clean filter in the repository's own config: the app's
# commit stores the filtered content; the app's status never runs it.
E="$SB/crypt"; newrepo "$E"
git -C "$E" config user.name Lab; git -C "$E" config user.email lab@example.test; git -C "$E" config commit.gpgsign false
printf 'secret.txt filter=crypt\n' > "$E/.gitattributes"
git -C "$E" config filter.crypt.clean "$(mk cryptclean "s/^/ENC:/")"
git -C "$E" config filter.crypt.smudge "$(mk cryptsmudge "s/^ENC://")"
git -C "$E" config filter.crypt.required true
printf 'hello\n' > "$E/secret.txt"
PE="$(jstr "$E")"
reset; pcall changes_stage "{\"repoPath\":$PE,\"paths\":[\".gitattributes\",\"secret.txt\"]}" >/dev/null
out="$(pcall changes_commit "{\"repoPath\":$PE,\"message\":\"add the secret\",\"amend\":false}")"
ok "the app's commit went through" "$(git -C "$E" log -1 --format=%s)" "add the secret"
ok "  the clean filter ran on staging" "$(fired)" "cryptclean"
ok "  the committed blob is the filtered content" "$(git -C "$E" cat-file -p HEAD:secret.txt)" "ENC:hello"
dirty "$E/secret.txt"
reset; pcall changes_repo_status "{\"repoPath\":$PE}" >/dev/null
ok "  and the app's status does not run that filter" "ran:[$(fired)]" "ran:[]"

# A husky style hooks folder in the repository's own config: the app's commit
# runs its pre-commit hook.
U="$SB/husky"; newrepo "$U"
git -C "$U" config user.name Lab; git -C "$U" config user.email lab@example.test; git -C "$U" config commit.gpgsign false
mkdir -p "$U/.husky"; printf '#!/bin/sh\necho "husky-pre-commit" >> "%s"\nexit 0\n' "$MARK" > "$U/.husky/pre-commit"; chmod +x "$U/.husky/pre-commit"
git -C "$U" config core.hooksPath .husky
printf 'x\n' > "$U/f.txt"
PU="$(jstr "$U")"
pcall changes_stage "{\"repoPath\":$PU,\"paths\":[\"f.txt\"]}" >/dev/null
reset; out="$(pcall changes_commit "{\"repoPath\":$PU,\"message\":\"with husky\",\"amend\":false}")"
ok "the app's commit runs a local core.hooksPath pre-commit hook" "$(fired)" "husky-pre-commit"
ok "  and commits" "$(git -C "$U" log -1 --format=%s)" "with husky"

# Downloading a submodule never runs the repository's update = !command.
reset; out="$(pcall changes_submodule_update "{\"repoPath\":$(jstr "$S"),\"paths\":[\"sub\"]}")"
ok "Download submodule: submodule.<name>.update = !command does not run" "ran:[$(fired)]" "ran:[]"
ok "  the recorded commit is checked out" "$(git -C "$S/sub" rev-parse HEAD 2>&1)" "$(git -C "$S0" rev-parse HEAD)"

# Going back to a state checks a submodule that is not downloaded out again
# from what the repository has: `--checkout`, so update = !command never runs.
PS="$(jstr "$S")"
out="$(pcall states_save "{\"repoPath\":$PS,\"slot\":\"1\",\"name\":\"with sub\",\"replace\":null}")"
ok "(a state of the superproject, its submodule included)" "$(printf '%s' "$out" | jqf value.ok) $(printf '%s' "$out" | jqf value.detail)" "True"
SOID="$(pcall states_list "{\"repoPath\":$PS}" | "$PY" -c 'import sys,json; print(json.load(sys.stdin)["value"]["states"][0]["oid"])')"
undownload() { git -C "$S" submodule deinit -q -f sub >/dev/null 2>&1; git -C "$S" config submodule.sub.update "!$(mk updatecmd)"; }
undownload
ok "(the submodule is not downloaded, and its update is !command)" "$([ -e "$S/sub/.git" ] && echo downloaded || echo not) $(git -C "$S" config submodule.sub.update)" "not !"
reset; out="$(pcall states_restore "{\"repoPath\":$PS,\"slot\":\"1\",\"oid\":\"$SOID\",\"newBranch\":null,\"allowPushed\":false}")"
ok "Go back to a state over a submodule not downloaded: update = !command does not run" "ran:[$(fired)]" "ran:[]"
ok "  the state came back, the submodule at its saved commit" "$(printf '%s' "$out" | jqf value.ok) $(git -C "$S/sub" rev-parse HEAD 2>&1)" "True $(git -C "$S0" rev-parse HEAD)"
undownload
reset; git -C "$S" submodule update --init --no-fetch -- sub >/dev/null 2>&1
ok "control: plain git submodule update --init --no-fetch runs that command" "$(fired)" "updatecmd"

# =============================================================================
section "5. The same, with the hostile folder inside a profile (lab 1's last rows)"
reset; profile LabAll "$SB"
ok "profile saved over the parent folder: nothing ran" "ran:[$(fired)]" "ran:[]"
passive_row "Repository picker, folder in a profile" history_list_repos  "{}" "$SB"
passive_row "the page selected the repository"       changes_repo_status "{\"repoPath\":$P}" "$H"

# =============================================================================
section "6. Repository text that would become a git option or a path outside the repository"
# Each case: plain git (or the old parsing) shows the text really does what
# the row guards against — a canary overwritten, a folder outside deleted —
# then the app refuses it and the canary and the folder stay.
canary() { printf 'precious\n' > "$1"; }
intact() { if [ "$(tr -d '\000' < "$1" 2>/dev/null)" = "precious" ]; then echo "canary unchanged"; else echo "canary OVERWRITTEN: $(head -c 60 "$1" 2>/dev/null | tr '\0\037\n' '0~ ')"; fi; }

# GIT-B-04: a saved state's `parent` line, written by hand, reached `git log`
# and `git diff-tree` as `--output=canary.txt` when the Changes tab listed the
# states (lab 2's case).
B4="$SB/states-hostile"; git init -q "$B4"; printf 'a\n' > "$B4/f.txt"; git -C "$B4" add f.txt
git -C "$B4" -c user.name=x -c user.email=x@x.test commit -qm x
B4C=$(git -C "$B4" rev-parse HEAD); B4T=$(git -C "$B4" rev-parse 'HEAD^{tree}')
B4S=$(printf 'tree %s\nparent --output=canary.txt\nparent %s\nauthor x <x@x.test> 1700000000 +0000\ncommitter x <x@x.test> 1700000000 +0000\n\nGitSwitch state: x\n\n{"gitswitch-state":1,"name":"x","branch":null,"saved":0}\n' "$B4T" "$B4C" | git -C "$B4" hash-object -t commit -w --literally --stdin)
# Written as a file, as an archive would carry it: `git update-ref` itself
# refuses a commit whose parents are not ids.
mkdir -p "$B4/.git/refs/gitswitch/states/main"; printf '%s\n' "$B4S" > "$B4/.git/refs/gitswitch/states/main/1"
ok "(the crafted state is there for the app to find)" "$(git -C "$B4" for-each-ref --format='%(refname) %(objectname)' refs/gitswitch/states/main/)" "refs/gitswitch/states/main/1 $B4S"
B4P="$(git -C "$B4" cat-file commit "$B4S" | sed -n 's/^parent //p' | head -1)"
canary "$B4/canary.txt"; git -C "$B4" log -1 --format=%h%x00%s "$B4P" >/dev/null 2>&1
ok "control: git prints the crafted parent as stored, and git log given it overwrites the canary" "$B4P / $(intact "$B4/canary.txt")" "--output=canary.txt / canary OVERWRITTEN"
canary "$B4/canary.txt"
out="$(pinvoke states_list "{\"repoPath\":$(jstr "$B4")}")"
ok "states_list over a crafted state: the canary stays" "$(intact "$B4/canary.txt")" "canary unchanged"
ok "  the list still answers, without the crafted state" "$(printf '%s' "$out" | jqf ok) $(printf '%s' "$out" | jqf value.states)" "True []"
out="$(pinvoke states_restore "{\"repoPath\":$(jstr "$B4"),\"slot\":\"1\",\"oid\":\"$B4S\",\"newBranch\":null,\"allowPushed\":false}")"
# git here cannot peel the crafted commit, so the slot reads as empty and the
# page is told the state is gone; a git that reads it gets read_saved's own
# refusal. Either way nothing is restored.
said="$(printf '%s' "$out" | jqf error) $(printf '%s' "$out" | jqf value.refusal.code)"
case "$said" in *"isn't a state GitSwitch saved"*|*no-state*) said="refused: $said" ;; esac
ok "Go back to the crafted state: refused" "$said" "refused: "
ok "  the canary stays, HEAD did not move, nothing was kept as 'Before going back'" "$(intact "$B4/canary.txt") $(git -C "$B4" rev-parse HEAD) $(git -C "$B4" for-each-ref refs/gitswitch/states/main/before | wc -l | tr -d ' ')" "canary unchanged $B4C 0"

# GIT-A-07: HEAD on a branch named `--output=canary.txt` (git lets a ref be
# called that); the History tab's sync line passed it to `git log`.
A7="$SB/head-hostile"; newrepo "$A7"
git -C "$A7" update-ref "refs/heads/--output=canary.txt" "$(git -C "$A7" rev-parse HEAD)"
git -C "$A7" symbolic-ref HEAD "refs/heads/--output=canary.txt"
canary "$A7/canary.txt"; git -C "$A7" log -1 --format=%h "$(git -C "$A7" rev-parse --abbrev-ref HEAD)" >/dev/null 2>&1
ok "control: plain git log given HEAD's branch name overwrites the canary" "$(intact "$A7/canary.txt")" "canary OVERWRITTEN"
canary "$A7/canary.txt"
out="$(pinvoke history_sync_status "{\"repoPath\":$(jstr "$A7"),\"branch\":\"--all\"}")"
ok "History sync line: the canary stays" "$(intact "$A7/canary.txt")" "canary unchanged"
ok "  and it is refused as an invalid revision" "ok=$(printf '%s' "$out" | jqf ok): $(printf '%s' "$out" | jqf error)" "Invalid revision: --output=canary.txt"
out="$(pinvoke history_branch_merges "{\"repoPath\":$(jstr "$A7"),\"branch\":\"--output=canary.txt\"}")"
ok "Branch details for that branch: the canary stays" "$(intact "$A7/canary.txt")" "canary unchanged"

# PARSE-R2-01: 0x1F in a commit subject moved its own words to where "Go back
# to this commit" read the parents, and they reached `git log` as options.
R1="$SB/subject-hostile"; newrepo "$R1"; R1P=$(git -C "$R1" rev-parse HEAD)
printf 'a\037b\037c\037--output=canary.txt\n' > "$SB/subject-msg.txt"
git -C "$R1" -c user.name=x -c user.email=x@x.test commit -q --allow-empty -F "$SB/subject-msg.txt"
R1Y=$(git -C "$R1" rev-parse HEAD)
OLDF="$(git -C "$R1" log -1 '--format=%H%x1f%h%x1f%s%x1f%an%x1f%cI%x1f%P' "$R1Y" | "$PY" -c 'import sys; print(sys.stdin.read().split("\x1f")[5].strip())')"
canary "$R1/canary.txt"; git -C "$R1" log --no-walk=unsorted --format=%H "$OLDF" >/dev/null 2>&1
ok "control: the old format read the subject's words as the parents, and git log given them overwrites the canary" "$OLDF / $(intact "$R1/canary.txt")" "--output=canary.txt / canary OVERWRITTEN"
canary "$R1/canary.txt"
out="$(pinvoke history_resolve "{\"repoPath\":$(jstr "$R1"),\"text\":\"$R1Y\"}")"
ok "Go back to this commit: the canary stays" "$(intact "$R1/canary.txt")" "canary unchanged"
ok "  its real parent is the one shown, and the subject is its subject" "$(printf '%s' "$out" | jqf value.parents | "$PY" -c 'import sys,json; print(",".join(p["oid"] for p in json.load(sys.stdin)))') $(printf '%s' "$out" | jqf value.is_merge)" "$R1P False"

# DEST-B-01: a submodule name `../../..` in .gitmodules steered the clearing of
# an interrupted download to the folder above the repository (lab 2's case).
dest_case() {  # dest_case <label> <submodule name> -> the probe's answer
  local D="$SB/dest-$1/a/b/c/outer"
  case "$D" in "$SB"/*) ;; *) echo "refusing: $D is outside the sandbox"; exit 1;; esac
  mkdir -p "$D"; printf 'keep me\n' > "$D/victim.txt"
  local R="$D/repo"; newrepo "$R"
  git -C "$R" update-index --add --cacheinfo 160000,1234567890123456789012345678901234567890,sub
  printf '[submodule "%s"]\n\tpath = sub\n\turl = https://example.invalid/x\n' "$2" > "$R/.gitmodules"
  git -C "$R" add .gitmodules; git -C "$R" -c user.name=x -c user.email=x@x.test commit -qm sub
  mkdir -p "$R/sub"; git init -q "$R/sub"; mkdir -p "$R/.git/modules"
  pinvoke changes_submodule_update "{\"repoPath\":$(jstr "$R"),\"paths\":[\"sub\"]}"
}
out="$(dest_case plain lib)"
ok "control: with a plain name, the interrupted download's leftovers are cleared (the delete is reached)" "$(printf '%s' "$out" | jqf value.detail)" "An interrupted download of sub was cleared"
out="$(dest_case planted "../../..")"
DP="$SB/dest-planted/a/b/c/outer"
ok "submodule name '../../..': the folder outside the repository stays" "$([ -f "$DP/victim.txt" ] && echo kept || echo DELETED)" "kept"
ok "  nothing of the submodule was cleared either" "$([ -d "$DP/repo/sub/.git" ] && echo kept || echo cleared)" "kept"
ok "  refused, saying why" "$(printf '%s' "$out" | jqf value.detail)" "not plain folder names"
ok "  and named as not downloaded" "$(printf '%s' "$out" | jqf value.submodules.failed_paths)" '["sub"]'

# DEST-B-01 again, with plain names: `.git/modules` itself is a link to the
# folder that holds the repository (an archive can carry one). The module
# folder `victim` resolves through it to an ordinary folder outside, which
# holds no commit and so reads as an interrupted download.
link_dir() {  # link_dir <target folder> <link>: a symbolic link; a junction on Windows (no developer mode needed)
  if command -v cygpath >/dev/null 2>&1; then MSYS2_ARG_CONV_EXCL='*' cmd /c mklink /J "$(cygpath -w "$2")" "$(cygpath -w "$1")" >/dev/null; else ln -s "$1" "$2"; fi
}
modules_case() {  # modules_case <label> <link|real> [fresh]: the folder <sandbox>/modules-<label>/x, with `repo` and `victim` in it
  local D="$SB/modules-$1/x"
  case "$D" in "$SB"/*) ;; *) echo "refusing: $D is outside the sandbox"; exit 1;; esac
  mkdir -p "$D/victim"; printf 'keep me\n' > "$D/victim/keep.txt"
  local R="$D/repo"; newrepo "$R"
  git -C "$R" update-index --add --cacheinfo 160000,1234567890123456789012345678901234567890,sub
  printf '[submodule "victim"]\n\tpath = sub\n\turl = https://example.invalid/x\n' > "$R/.gitmodules"
  git -C "$R" add .gitmodules; git -C "$R" -c user.name=x -c user.email=x@x.test commit -qm sub
  # fresh: never downloaded, nothing in `sub`
  [ "${3:-}" = fresh ] || { mkdir -p "$R/sub"; printf 'gitdir: ../.git/modules/victim\n' > "$R/sub/.git"; }
  if [ "$2" = link ]; then link_dir "$D" "$R/.git/modules"; else mkdir -p "$R/.git/modules/victim"; fi
}
download() { pinvoke changes_submodule_update "{\"repoPath\":$(jstr "$1"),\"paths\":[\"sub\"]}"; }
modules_case real real; DR="$SB/modules-real/x"
out="$(download "$DR/repo")"
ok "control: a real, commitless .git/modules/victim is cleared (the same row reaches the delete)" "$(printf '%s' "$out" | jqf value.detail)" "An interrupted download of sub was cleared"
ok "  the module folder is gone" "$([ -d "$DR/repo/.git/modules/victim" ] && echo kept || echo cleared)" "cleared"
modules_case link link; DL="$SB/modules-link/x"
ok "(.git/modules is a link to the folder outside: victim is reached through it)" "$(cat "$DL/repo/.git/modules/victim/keep.txt" 2>&1)" "keep me"
out="$(download "$DL/repo")"
ok ".git/modules a link out of the repository: the folder outside and its file stay" "$([ -f "$DL/victim/keep.txt" ] && echo kept || echo DELETED)" "kept"
ok "  nothing of the submodule was cleared either" "$([ -f "$DL/repo/sub/.git" ] && echo kept || echo cleared)" "kept"
ok "  and it is named as not downloaded" "$(printf '%s' "$out" | jqf value.submodules.failed_paths)" '["sub"]'
ok "  git wrote nothing through the link either: the folder outside holds only its file" "$([ "$(ls -A "$DL/victim")" = keep.txt ] && echo only-keep.txt || ls -A "$DL/victim" | tr '\n' ' ')" "only-keep.txt"
ok "  saying why" "$(printf '%s' "$out" | jqf value.detail)" ".git/modules leads outside its git folder"
# The same link with the submodule never downloaded: nothing to clear, but
# git's own submodule update, finding a module folder there, removes its
# `index` and writes `core.worktree` into its `config`. It is never started.
modules_case fresh link fresh; DF="$SB/modules-fresh/x"
out="$(download "$DF/repo")"
ok ".git/modules a link out, the submodule never downloaded: git wrote nothing through the link" "$([ "$(ls -A "$DF/victim")" = keep.txt ] && echo only-keep.txt || ls -A "$DF/victim" | tr '\n' ' ')" "only-keep.txt"
ok "  named as not downloaded" "$(printf '%s' "$out" | jqf value.submodules.failed_paths)" '["sub"]'
ok "  saying why" "$(printf '%s' "$out" | jqf value.detail)" ".git/modules leads outside its git folder"

verify_result
