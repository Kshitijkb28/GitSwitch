#!/bin/bash
# Submodule reporting, on the shape that breaks `git submodule status`: a
# superproject with one mapped submodule AND one gitlink that has no
# .gitmodules entry. Throwaway repos, isolated HOME, no network.
set -u
source "$(dirname "$0")/lib.sh"
verify_init subs "Sub Tester" "sub@example.test"

sub() { # json on stdin, <path> <field>
  python3 -c "
import sys,json
d=json.load(sys.stdin)
m=[x for x in d if x['path']=='$1']
print(json.dumps(m[0].get('$2')) if m else 'MISSING')
"; }

# --- three submodule upstreams ----------------------------------------------
for name in mapped orphan moved; do
  git init -q --bare "$SB/$name.git"
  git clone -q "$SB/$name.git" "$SB/${name}_work" 2>/dev/null
  cd "$SB/${name}_work"
  echo "$name v1" > file.txt
  git add -A >/dev/null; git commit -qm "$name initial"; git push -q origin main 2>/dev/null
done

# --- the superproject -------------------------------------------------------
git init -q --bare "$SB/super.git"
git clone -q "$SB/super.git" "$SB/super" 2>/dev/null
cd "$SB/super"
echo root > README.md; git add -A >/dev/null; git commit -qm "root"
git submodule add -q "$SB/mapped.git" mapped 2>/dev/null
git submodule add -q "$SB/orphan.git" orphan 2>/dev/null
git submodule add -q "$SB/moved.git" moved 2>/dev/null
git add -A >/dev/null; git commit -qm "add three submodules"
# Strip 'orphan' from .gitmodules but keep its gitlink — exactly argos's shape.
git config -f .gitmodules --remove-section submodule.orphan
git add .gitmodules >/dev/null; git commit -qm "drop orphan mapping"

section "The shape that breaks git's own tooling"
OUT=$(git submodule status 2>&1); RC=$?
ok "git submodule status really does fatal here" "$OUT" "no submodule mapping found"
ok "  and it aborts rather than listing the rest" "rc=$RC" "rc=128"

section "GitSwitch lists every gitlink anyway"
OUT=$(probe submodules_list "$SB/super")
ok "all three are reported" "$(printf '%s' "$OUT" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)))')" "3"
ok "  the mapped one is named" "$(printf '%s' "$OUT" | sub mapped name)" "mapped"
ok "  the unmapped one is still listed" "$(printf '%s' "$OUT" | sub orphan path)" "orphan"
ok "  and is flagged as having no entry" "$(printf '%s' "$OUT" | sub orphan listed)" "false"
ok "  clean submodules say so" "$(printf '%s' "$OUT" | sub mapped summary)" "up to date with what this repo records"

section "A submodule that moved forward"
cd "$SB/moved_work"
echo "moved v2" >> file.txt; git commit -qam "second"
echo "moved v3" >> file.txt; git commit -qam "third"
git push -q origin main 2>/dev/null
cd "$SB/super/moved"; git fetch -q origin 2>/dev/null; git checkout -q origin/main 2>/dev/null
cd "$SB/super"
OUT=$(probe submodules_list "$SB/super")
ok "the move is measured, not guessed" "$(printf '%s' "$OUT" | sub moved ahead)" "2"
ok "  and described in words" "$(printf '%s' "$OUT" | sub moved summary)" "moved 2 commits ahead"
ok "  with the commit subjects" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin); m=[x for x in d if x['path']=='moved'][0]
print(','.join(c['subject'] for c in m['moved_commits']))")" "third"
ok "  state is 'moved'" "$(printf '%s' "$OUT" | sub moved state)" "moved"
ok "  recorded and checked-out differ" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin); m=[x for x in d if x['path']=='moved'][0]
print('differ' if m['recorded_short'] != m['actual_short'] else 'same')")" "differ"

section "A submodule with its own uncommitted work"
echo "local edit" >> "$SB/super/mapped/file.txt"
printf 'new\n' > "$SB/super/mapped/untracked.txt"
OUT=$(probe submodules_list "$SB/super")
ok "changed files inside are counted" "$(printf '%s' "$OUT" | sub mapped dirty_tracked)" "1"
ok "untracked files inside are counted" "$(printf '%s' "$OUT" | sub mapped dirty_untracked)" "1"
ok "  and stated in the summary" "$(printf '%s' "$OUT" | sub mapped summary)" "1 changed file and 1 untracked file inside"
ok "  state is 'dirty'" "$(printf '%s' "$OUT" | sub mapped state)" "dirty"

section "An uninitialised submodule"
rm -rf "$SB/super/orphan"
mkdir -p "$SB/super/orphan"
OUT=$(probe submodules_list "$SB/super")
ok "an empty gitlink dir is reported as not initialised" "$(printf '%s' "$OUT" | sub orphan state)" "unmapped"
ok "  and says why git can't fetch it" "$(printf '%s' "$OUT" | sub orphan summary)" "no entry in .gitmodules"
ok "  it is not claimed to be initialised" "$(printf '%s' "$OUT" | sub orphan initialised)" "false"

section "The superproject's own file list no longer lies"
OUT=$(probe status "$SB/super")
ok "a submodule row carries no fake line counts" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
subs=[e for e in d['entries'] if e['is_submodule']]
print('none' if all(e['unstaged_added'] is None and e['staged_added'] is None for e in subs) else 'FAKE COUNTS')")" "none"
ok "  and it keeps the recorded commit" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
subs=[e for e in d['entries'] if e['is_submodule']]
print('yes' if subs and all(e['recorded_oid'] for e in subs) else 'no')")" "yes"

section "Clicking a submodule shows the pointer move, not 'nothing changed'"
OUT=$(probe diff "$SB/super" "moved,unstaged")
ok "the diff names both commits" "$(printf '%s' "$OUT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(' '.join(l['text'] for l in d['lines'] if 'Subproject' in l['text']))")" "Subproject commit"
ok "  and does not claim there are no changes" "$(printf '%s' "$OUT" | python3 -c "
import sys,json;print(json.load(sys.stdin)['empty_reason'])")" "None"

section "Update still works on this repo"
OUT=$(probe submodule "$SB/super")
ok "the unmapped gitlink is reported, not fatal" "$OUT" "up to date"
ok "  and the count of unfetchable ones is honest" "$(printf '%s' "$OUT" | jqf submodules.unlisted)" "1"

verify_result
