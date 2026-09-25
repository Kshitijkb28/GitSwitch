# Sync-with-upstream facts for repositories WITH submodules

Controlled git experiments settling the design of GitSwitch's "sync with upstream,
keeping my commits on top" operation. Everything ran in a throwaway sandbox
(`$TMPDIR/gitswitch-plan-sync`, `HOME` redirected, `GIT_CONFIG_NOSYSTEM=1`, no network,
"remotes" = local bare repos). The sandbox was deleted afterwards.

- Date: 2026-09-23
- Git: `git version 2.50.1 (Apple Git-155)`
- Sandbox `~/.gitconfig`: `user.name=Sync Tester`, `user.email=sync@example.test`,
  `init.defaultBranch=main`, `protocol.file.allow=always` (nothing else).
- Environment on every command: `GIT_EDITOR=true GIT_SEQUENCE_EDITOR=true LC_ALL=C GIT_TERMINAL_PROMPT=0`
- Pin set (`$PIN`) used on every superproject/submodule command unless a question says otherwise:
  `-c submodule.recurse=false -c fetch.recurseSubmodules=false -c rebase.updateRefs=false -c rebase.backend=merge -c rebase.rebaseMerges=false -c color.ui=never`
- `local` = `$SB/local`, `A`/`B` = `$SB/local/A`, `$SB/local/B`. Paths below are abbreviated.
- Before every strategy `local` was restored from `local.pristine` and `status --porcelain`
  of super and A was checked against the frozen capture (printed as `[restore] OK`).
- `hint:` lines are elided from outputs where noted.

## Fixture shape

```
super.git  A.git  B.git                          (bare "remotes")
author/A, author/B, author/super                  (created the initial history, pushed)
local/     git clone --recurse-submodules super.git, made BEFORE upstream's work
upstream/  another clone; produced upstream's work and pushed it
```

Commits (short SHAs are stable for the whole run):

| repo  | commit  | subject                                          | who       |
|-------|---------|--------------------------------------------------|-----------|
| super | 4016125 | super: initial (README.md, shared.txt 3 lines)   | author    |
| super | b726cbe | super: add submodules A and B  (clone point)     | author    |
| super | 9597d3e | bump A            (A -> 5e5f8ab)                 | upstream  |
| super | b1a30b5 | upstream: add upstream.txt, edit shared line 2   | upstream  |
| super | c0e382a | bump B            (B -> c391774) = origin/main   | upstream  |
| super | d24b9e2 | seal work         (A -> 6373153)                 | local     |
| super | c1a2b58 | local unrelated   (adds local.txt) = local HEAD  | local     |
| A     | 94d3a48 | A: initial  (clone point; merge base)            | author    |
| A     | 99abee6 | A: upstream 1  (appends "A line 2 (upstream)")   | upstream  |
| A     | 5e5f8ab | A: upstream 2  = A's origin/main = origin/main:A | upstream  |
| A     | 6373153 | A: local work  (adds local.txt) = HEAD:A (S1)    | local     |
| B     | 37c40e1 | B: initial  = HEAD:B                              | author    |
| B     | c391774 | B: upstream 1  = B's origin/main = origin/main:B | upstream  |

`.gitmodules` (both entries): `path`, `url = $SB/A.git` (path URL), `branch = main`, `update = merge`.

Uncommitted state in `local` (the frozen capture):

```
$ git -C local status --porcelain          $ git -C local/A status --porcelain
 M A                                        M A.txt
 M README.md                               ?? untracked-A.txt
?? untracked-super.txt
```

(`README.md` appended a line; `A/A.txt` appended "A line 1b (local uncommitted)";
B is clean and on `main`.)

Facts recorded while building the fixture:

```
$ git clone -q --recurse-submodules "$SB/super.git" "$SB/local"
$ git -C local/A symbolic-ref -q HEAD ; echo $?            -> (nothing) 1   => A is DETACHED after clone
$ git -C local/A status -sb | head -1                      -> ## HEAD (no branch)     (same for B)
$ cat local/A/.git                                         -> gitdir: ../.git/modules/A
$ git -C local/A symbolic-ref refs/remotes/origin/HEAD     -> refs/remotes/origin/main
$ git -C local/A branch -a
* (HEAD detached at 94d3a48)
  main
  remotes/origin/HEAD -> origin/main
  remotes/origin/main
$ git -C local config --get-regexp '^submodule\.'
submodule.active .
submodule.A.url /.../A.git
submodule.A.update merge          <= .gitmodules' "update = merge" IS copied into .git/config at clone
submodule.B.url /.../B.git
submodule.B.update merge
$ git -C local submodule status
 94d3a4871f234ef7b6df3af4c60060dd042700e9 A (heads/main)     <= "(heads/main)" is shown even though A is detached
 37c40e1ee5191cba65cfe674834a9dd434ca967f B (heads/main)
$ git -C local submodule foreach -q 'git checkout -q main'  -> A and B now "## main...origin/main"
```

Shell note: the harness shell was zsh, so `$PIN` had to be an array (`"${PIN[@]}"`); an
unquoted scalar `$PIN` is passed as ONE word by zsh and git says `unknown option: -c submodule.recurse=false ...`.

---

## Q1 - Assessment without side effects

```
$ git -C local  $PIN fetch --quiet --no-recurse-submodules -- origin   -> exit 0
$ git -C local/A $PIN fetch --quiet -- origin                          -> exit 0
$ git -C local/B $PIN fetch --quiet -- origin                          -> exit 0
(status --porcelain of super and A unchanged afterwards)

$ git -C local rev-list --left-right --count 'HEAD...@{u}'      -> 2	3
$ git -C local log --oneline '@{u}..HEAD'
c1a2b58 local unrelated
d24b9e2 seal work
$ git -C local log --oneline 'HEAD..@{u}'
c0e382a bump B
b1a30b5 upstream: add upstream.txt, edit shared line 2
9597d3e bump A

$ git -C local/A rev-list --left-right --count 'HEAD...@{u}'    -> 1	2
$ git -C local/A log --oneline '@{u}..HEAD'                     -> 6373153 A: local work
$ git -C local/B rev-list --left-right --count 'HEAD...@{u}'    -> 0	1
$ git -C local/B log --oneline '@{u}..HEAD'                     -> (empty, exit 0)

$ git -C local rev-parse HEAD:A origin/main:A
6373153df4f7d6f74209dd07e2d7af908919cdca      (recorded)
5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795      (target)
$ git -C local/A rev-parse HEAD origin/main
6373153df4f7d6f74209dd07e2d7af908919cdca      (head)
5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795      (tip)
$ git -C local rev-parse HEAD:B origin/main:B
37c40e1ee5191cba65cfe674834a9dd434ca967f
c391774105c5fe2ee8ae55af6518239be4ad20f8
$ git -C local/B rev-parse HEAD origin/main
37c40e1ee5191cba65cfe674834a9dd434ca967f
c391774105c5fe2ee8ae55af6518239be4ad20f8

A ancestry (head=6373153 target=5e5f8ab tip=5e5f8ab):
$ git -C local/A merge-base --is-ancestor <head>   <target>  -> exit 1   (A HEAD  -> target : no)
$ git -C local/A merge-base --is-ancestor <target> <head>    -> exit 1   (target -> A HEAD : no)   => DIVERGED
$ git -C local/A merge-base --is-ancestor <target> <tip>     -> exit 0   (target -> tip)
$ git -C local/A merge-base --is-ancestor <tip>    <target>  -> exit 0   (tip -> target; equal)
$ git -C local/A merge-base <head> <target>                  -> 94d3a4871f234ef7b6df3af4c60060dd042700e9
B ancestry (head=37c40e1 target=c391774 tip=c391774):
  B HEAD -> target exit 0 ; target -> B HEAD exit 1 ; target -> tip exit 0 ; tip -> target exit 0   => B simply BEHIND

$ git -C local cat-file -t 5e5f8ab...   -> fatal: git cat-file: could not get object info (exit 128)
   (the superproject object store never holds submodule commits; ancestry MUST be asked inside local/A)
```

Recursion test on throwaway copies of the pristine clone (A's origin/main was 94d3a48 before):

```
$ git -C copy fetch --recurse-submodules=on-demand origin
From .../super
   b726cbe..c0e382a  main       -> origin/main
Fetching submodule A
From .../A
   94d3a48..5e5f8ab  main       -> origin/main
Fetching submodule B
From .../B
   37c40e1..c391774  main       -> origin/main
$ git -C copy/A rev-parse origin/main        -> 5e5f8ab...   (advanced)

$ git -C copy fetch --recurse-submodules=yes origin        -> identical output, A and B advanced
$ git -C copy fetch --no-recurse-submodules origin         -> only super; A origin/main stays 94d3a48
$ git -C copy fetch origin        (NO flag, NO config)     -> "Fetching submodule A / B": recursed on-demand, A advanced
```

Conclusion: the three pinned fetches assess everything with no side effects; A is diverged (needs a rebase),
B is behind (fast-forward). Both `on-demand` and `yes` DO recurse into path-URL submodules and advance
`origin/main` inside A/B - and so does a plain `git fetch` with no flag (on-demand is the default), so
the `fetch.recurseSubmodules=false` pin / `--no-recurse-submodules` is required for "super only" fetches.

---

## Q2 - Naive `pull --rebase --autostash --recurse-submodules`

```
$ git -C local $PIN pull --rebase --autostash --recurse-submodules origin main
From .../super
 * branch            main       -> FETCH_HEAD
   b726cbe..c0e382a  main       -> origin/main
Fetching submodule A
From .../A
   94d3a48..5e5f8ab  main       -> origin/main
Fetching submodule B
From .../B
   37c40e1..c391774  main       -> origin/main
fatal: cannot rebase with locally recorded submodule modifications
exit=128
$ git -C local status --porcelain=v2
1 .M S.MU 160000 160000 160000 6373153... 6373153... A
1 .M N... 100644 100644 100644 36d77a8... 36d77a8... README.md
? untracked-super.txt
$ git -C local submodule status
 6373153df4f7d6f74209dd07e2d7af908919cdca A (heads/main)
 37c40e1ee5191cba65cfe674834a9dd434ca967f B (heads/main)
$ git -C local/A log --oneline --all --decorate -8
6373153 (HEAD -> main) A: local work
5e5f8ab (origin/main, origin/HEAD) A: upstream 2
99abee6 A: upstream 1
94d3a48 A: initial
(A: " M A.txt / ?? untracked-A.txt", on refs/heads/main; B unchanged on main at 37c40e1)
$ git -C local stash list        -> (empty)
$ git -C local ls-files -u       -> (empty)
rebase-merge dir exists?         -> no
$ git -C local $PIN rebase --abort
fatal: no rebase in progress
exit=128
after: status of super and A == capture, HEAD c1a2b58 unchanged
```

Extra (Q2b) - same pull WITHOUT `--recurse-submodules` (pinned, so A was NOT fetched):

```
$ git -C local $PIN pull --rebase --autostash origin main
...
Created autostash: 948eb63
Rebasing (1/2)hint: Recursive merging with submodules currently only supports trivial cases.
...
Failed to merge submodule A (commits not present)
CONFLICT (submodule): Merge conflict in A
error: could not apply d24b9e2... seal work
Could not apply d24b9e2... # seal work
exit=1
$ git -C local ls-files -u
160000 94d3a4871f234ef7b6df3af4c60060dd042700e9 1	A
160000 5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795 2	A
160000 6373153df4f7d6f74209dd07e2d7af908919cdca 3	A
$ git -C local/A rev-parse origin/main   -> 94d3a48...  (pull did not fetch into A)
$ git -C local $PIN rebase --abort       -> Applied autostash. exit 0
```

Conclusion: the naive pull with `--recurse-submodules` refuses before doing anything
("cannot rebase with locally recorded submodule modifications", exit 128; only the fetches happened).
Without `--recurse-submodules` it becomes exactly the Q3 rebase (gitlink stop), with the extra
"(commits not present)" wrinkle when A has not been fetched. The app must drive the rebase itself.

---

## Q3 - Superproject rebase; gitlink stop resolved by rebasing A DURING the stop

Pre: super HEAD c1a2b58, A HEAD 6373153, B HEAD 37c40e1; the three Q1 fetches done.

```
$ git -C local $PIN rebase --merge --autostash refs/remotes/origin/main
Created autostash: 9ad6e8d
Rebasing (1/2)hint: Recursive merging with submodules currently only supports trivial cases.
hint: Please manually handle the merging of each conflicted submodule.
hint: This can be accomplished with the following steps:
hint:  - go to submodule (A), and either merge commit 6373153
hint:    or update to an existing commit which has merged those changes
hint:  - come back to superproject and run:
hint:
hint:       git add A
hint:
hint:    to record the above merge or update
hint:  - resolve any other conflicts in the superproject
hint:  - commit the resulting index in the superproject
hint:
hint: Disable this message with "git config set advice.submoduleMergeConflict false"
Failed to merge submodule A
CONFLICT (submodule): Merge conflict in A
error: could not apply d24b9e2... seal work
hint: Resolve all conflicts manually, mark them as resolved with
hint: "git add/rm <conflicted_files>", then run "git rebase --continue".
hint: You can instead skip this commit: run "git rebase --skip".
hint: To abort and get back to the state before "git rebase", run "git rebase --abort".
hint: Disable this message with "git config set advice.mergeConflict false"
Could not apply d24b9e2... # seal work
exit=1

$ git -C local ls-files -u -z | tr '\0' '\n'
160000 94d3a4871f234ef7b6df3af4c60060dd042700e9 1	A        (stage 1 = merge base)
160000 5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795 2	A        (stage 2 = upstream = "ours" during rebase)
160000 6373153df4f7d6f74209dd07e2d7af908919cdca 3	A        (stage 3 = the commit being replayed = own)

$ git -C local status --porcelain=v2 -z | tr '\0' '\n'
1 .M SC.. 160000 160000 160000 c391774105c5fe2ee8ae55af6518239be4ad20f8 c391774105c5fe2ee8ae55af6518239be4ad20f8 B
u UU SCMU 160000 160000 160000 160000 94d3a4871f234ef7b6df3af4c60060dd042700e9 5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795 6373153df4f7d6f74209dd07e2d7af908919cdca A
? untracked-super.txt
   (B shows "SC.." = its worktree HEAD 37c40e1 differs from the checked-out commit's gitlink c391774;
    A's sub-state "SCMU" = submodule, commit differs, modified content, untracked content)

$ git -C local diff --name-only --diff-filter=U        -> A
$ git -C local rev-parse --path-format=absolute --git-path rebase-merge
/private/var/.../local/.git/rebase-merge                -> test -d: EXISTS
$ cat rebase-merge/done                -> pick d24b9e2ea6b69eab952296b1f0df5ba68707361a # seal work
$ cat rebase-merge/stopped-sha         -> d24b9e2ea6b69eab952296b1f0df5ba68707361a
$ cat rebase-merge/git-rebase-todo     -> pick c1a2b584fbb46c49317cd980b10fda07fa0c4253 # local unrelated
head-name=refs/heads/main  onto=c0e382a...  orig-head=c1a2b58...  autostash=9ad6e8daacf2af6aed40770c5b6f706fa99abfbb
$ ls rebase-merge
author-script autostash done drop_redundant_commits end git-rebase-todo git-rebase-todo.backup
head-name interactive message msgnum no-reschedule-failed-exec onto orig-head patch stopped-sha
super HEAD during stop: c0e382a (= onto)   A HEAD: 6373153 (untouched)   $ git -C local stash list -> (EMPTY)
A status during stop: " M A.txt / ?? untracked-A.txt" (untouched)   untracked-super.txt present
README.md during stop: "# super" only  (the autostash removed the working-tree edit)
```

Resolution:

```
stage2=5e5f8ab... stage3=6373153...
$ git -C local/A rev-parse HEAD                                 -> 6373153...   == stage3: YES
$ git -C local/A $PIN rebase --merge --autostash 5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795
Created autostash: a654aa6
Rebasing (1/1)Applying autostash resulted in conflicts.
Your changes are safe in the stash.
You can run "git stash pop" or "git stash drop" at any time.
Successfully rebased and updated refs/heads/main.
exit=0
$ git -C local/A log --oneline -5
7aa2c7f A: local work
5e5f8ab A: upstream 2
99abee6 A: upstream 1
94d3a48 A: initial
$ git -C local/A status --porcelain ; stash list ; symbolic-ref HEAD
UU A.txt
?? untracked-A.txt
stash@{0}: autostash
refs/heads/main
$ git -C local $PIN add -- A                                    -> exit 0   (accepted although A's worktree has UU)
$ git -C local ls-files -u                                      -> (empty)
$ git -C local ls-files -s -- A                                 -> 160000 7aa2c7ffe0180bfeca7b273eed98f0ffbce2ce7b 0	A
$ git -C local $PIN rebase --continue
[detached HEAD d7f612f] seal work
 1 file changed, 1 insertion(+), 1 deletion(-)
Rebasing (2/2)Applied autostash.
Successfully rebased and updated refs/heads/main.
exit=0
$ git -C local log --oneline -6
4cfd9f2 local unrelated
d7f612f seal work
c0e382a bump B
b1a30b5 upstream: add upstream.txt, edit shared line 2
9597d3e bump A
b726cbe super: add submodules A and B
```

Align B:

```
$ git -C local rev-parse HEAD:B                                            -> c391774105c5fe2ee8ae55af6518239be4ad20f8
$ git -C local/B merge-base --is-ancestor HEAD c391774105c5fe2ee8ae55af6518239be4ad20f8   -> exit 0
$ git -C local/B $PIN merge --ff-only --no-edit --no-stat -- c391774105c5fe2ee8ae55af6518239be4ad20f8
Updating 37c40e1..c391774
Fast-forward
exit=0        B: refs/heads/main @ c391774
```

Verify:

```
$ git -C local rev-list --left-right --count 'HEAD...@{u}'     -> 2	0
$ git -C local log --oneline '@{u}..HEAD'
4cfd9f2 local unrelated
d7f612f seal work                                               (own commits on top, original order)
A: rev-list -> 1	0 ; log @{u}..HEAD -> 7aa2c7f A: local work
B: rev-list -> 0	0
$ git -C local submodule status
 7aa2c7ffe0180bfeca7b273eed98f0ffbce2ce7b A (heads/main)
 c391774105c5fe2ee8ae55af6518239be4ad20f8 B (heads/main)        (no + / -)
$ git -C local status --porcelain          -> " M A / M README.md / ?? untracked-super.txt"   IDENTICAL to capture
$ git -C local/A status --porcelain        -> "UU A.txt / ?? untracked-A.txt"                 DIFFERS from capture (" M A.txt")
untracked-super.txt and A/untracked-A.txt present
stash lists: super (empty), A: "stash@{0}: autostash", B (empty)
$ git -C local ls-files -u                 -> (empty)
$ git -C local/A ls-files -u
100644 18c9b49... 1	A.txt
100644 d7c001a... 2	A.txt
100644 933f067... 3	A.txt
rebase-merge dir exists? no
README.md = "# super\nlocal README edit"   shared.txt = "shared 1 / shared 2 (upstream) / shared 3"
A/A.txt:
A line 1
<<<<<<< Updated upstream
A line 2 (upstream)
=======
A line 1b (local uncommitted)
>>>>>>> Stashed changes
```

Q3c - same run with A's uncommitted edit in `A/local.txt` instead of `A/A.txt` (no textual overlap with upstream):

```
$ git -C local/A $PIN rebase --merge --autostash 5e5f8ab...
Created autostash: 167f69e
Rebasing (1/1)Applied autostash.
Successfully rebased and updated refs/heads/main.
A status after: " M local.txt / ?? untracked-A.txt", stash list empty, ls-files -u empty
$ git -C local $PIN rebase --continue -> [detached HEAD 0d8f478] seal work ... Applied autostash. Successfully rebased and updated refs/heads/main.
B ff: Updating 37c40e1..c391774 Fast-forward
counts: super 2/0, A 1/0, B 0/0 ; submodule status has no +/- ; super status == capture ; A status == its own pre-state ;
untracked present ; all stash lists empty ; all ls-files -u empty
```

Conclusion: rebase-during-stop WORKS: the superproject stop is a normal 3-stage index conflict on the
gitlink (mode 160000), A's worktree is untouched by the superproject rebase, A's HEAD equals stage 3,
`rebase --merge --autostash <stage2>` inside A followed by `git add -- A` and `rebase --continue`
finishes with own commits on top and B fast-forwardable. Two caveats discovered:
(1) the superproject's autostash lives in `.git/rebase-merge/autostash` - `stash list` is EMPTY during
the stop, so "stash list empty" is not evidence that nothing is stashed; (2) the submodule's own
`--autostash` can fail to re-apply (here the fixture's `A.txt` edit touches the same spot as upstream):
git then leaves `stash@{0}: autostash` + `UU A.txt` in A and still reports the A rebase as successful
(exit 0), and `git add -- A` happily records A's HEAD regardless of A's worktree. The app MUST check
`git -C A ls-files -u` and `git -C A stash list` after the submodule rebase.

---

## Q4 - A1 case: A's checkout differs from the recorded gitlink, no superproject commit

```
$ git -C local/A commit --allow-empty -m extra              -> d039558 extra (on top of 6373153)
$ git -C local rev-parse HEAD:A     -> 6373153...     $ git -C local/A rev-parse HEAD -> d039558...
$ git -C local status --porcelain=v2 | grep ' A$'
1 .M SCMU 160000 160000 160000 6373153... 6373153... A       (worktree A differs: "new commits")

$ git -C local $PIN rebase --merge --autostash refs/remotes/origin/main
Created autostash: 47c8518
Rebasing (1/2)hint: ...(hint block identical to Q3)...
Failed to merge submodule A
CONFLICT (submodule): Merge conflict in A
error: could not apply d24b9e2... seal work
Could not apply d24b9e2... # seal work
exit=1
$ git -C local ls-files -u
160000 94d3a4871f234ef7b6df3af4c60060dd042700e9 1	A
160000 5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795 2	A
160000 6373153df4f7d6f74209dd07e2d7af908919cdca 3	A         (stage 3 = the COMMIT's gitlink, not A's HEAD)
rebase-merge exists: yes ; done: pick d24b9e2... # seal work
super HEAD c0e382a ; A HEAD d039558 (unchanged) ; stash list empty
$ git -C local $PIN rebase --abort         -> Applied autostash. exit 0
after abort: HEAD c1a2b58 ; status == capture ; A HEAD still d039558
```

Q4b - A's HEAD BEHIND the recorded gitlink (A detached at 94d3a48 while HEAD:A = 6373153):

```
$ git -C local $PIN rebase --merge --autostash refs/remotes/origin/main   -> same stop, exit 1
$ git -C local ls-files -u          -> same three stages (94d3a48 / 5e5f8ab / 6373153)
A HEAD after stop: 94d3a48 (not moved)
$ git -C local $PIN rebase --abort  -> Applied autostash. exit 0
```

Conclusion: git does NOT fail at the first checkout and does NOT notice the mismatch at the pick either.
With `submodule.recurse=false` the superproject rebase never touches submodule worktrees; the stop and its
stages are computed purely from the commits' gitlinks. `ls-files -u` is NOT empty (same 3 stages as Q3) and
A's HEAD is left wherever it was, so stage 3 != A HEAD. The `head == recorded` precondition is entirely the
app's responsibility (check it in the assessment step and again at every stop).

---

## Q5 - Autostash pop conflict (uncommitted edit on `shared.txt` line 2)

Setup: README edit reverted; `shared.txt` line 2 -> "shared 2 (LOCAL uncommitted)"; A's uncommitted edit
moved to `A/local.txt` to isolate the superproject behaviour. super status: ` M A / M shared.txt / ?? untracked-super.txt`.

```
$ git -C local $PIN rebase --merge --autostash refs/remotes/origin/main     (hints elided)
Created autostash: 3f05a72
Rebasing (1/2)Failed to merge submodule A
CONFLICT (submodule): Merge conflict in A
error: could not apply d24b9e2... seal work
Could not apply d24b9e2... # seal work
exit=1
(A stop resolved as in Q3: A rebase -> "Applied autostash. Successfully rebased and updated refs/heads/main."; git add -- A)
$ git -C local $PIN rebase --continue
[detached HEAD fe6b7ab] seal work
 1 file changed, 1 insertion(+), 1 deletion(-)
Rebasing (2/2)Applying autostash resulted in conflicts.
Your changes are safe in the stash.
You can run "git stash pop" or "git stash drop" at any time.
Successfully rebased and updated refs/heads/main.
exit=0                                                      <= exit 0 despite the conflict
$ git -C local ls-files -u
100644 e21619fa2d87272ee453301c44cf080ab10e7c72 1	shared.txt
100644 af88be3ce7f43a5622085e93e9539d2253cf3de9 2	shared.txt
100644 4f536ecfa37e06d71965b091c58a9c5b4778213c 3	shared.txt
$ git -C local status --porcelain=v2
1 .M S.MU 160000 160000 160000 d4278f1... d4278f1... A
1 .M SC.. 160000 160000 160000 c391774... c391774... B
u UU N... 100644 100644 100644 100644 e21619f... af88be3... 4f536ec... shared.txt
? untracked-super.txt
markers: rebase-merge: no   MERGE_HEAD: no   rebase-apply: no       <= NO operation in progress
$ git -C local stash list                  -> stash@{0}: autostash
shared.txt:
shared 1
<<<<<<< Updated upstream
shared 2 (upstream)
=======
shared 2 (LOCAL uncommitted)
>>>>>>> Stashed changes
shared 3
$ git -C local log --oneline '@{u}..HEAD'  -> a00ebd6 local unrelated / fe6b7ab seal work   (the rebase itself finished)

Recovery:
$ git -C local $PIN reset --hard           -> HEAD is now at a00ebd6 local unrelated ; exit 0
   status: " M A / M B / ?? untracked-super.txt"  (untracked kept; A/B lines = submodule worktrees, untouched)
$ git -C local $PIN stash pop
Auto-merging shared.txt
CONFLICT (content): Merge conflict in shared.txt
On branch main
Your branch is ahead of 'origin/main' by 2 commits.
...
Unmerged paths:  both modified:   shared.txt
...
The stash entry is kept in case you need it again.
exit=1
   after: "UU shared.txt", stash@{0}: autostash still present, same markers in shared.txt
$ git -C local $PIN reset --hard -q ; git -C local stash show -p 'stash@{0}'
diff --git a/shared.txt b/shared.txt
@@ -1,3 +1,3 @@
 shared 1
-shared 2
+shared 2 (LOCAL uncommitted)
 shared 3
```

Conclusion: an autostash that fails to apply does NOT fail the rebase (exit 0, "Successfully rebased"), leaves
NO operation marker (no `rebase-merge`, no `MERGE_HEAD`), leaves `UU shared.txt` in the index plus conflict
markers in the file, and keeps the stash as `stash@{0}: autostash`. Detection = `ls-files -u` non-empty AND/OR
`stash list` containing "autostash" right after a successful rebase. `git reset --hard` cleanly returns to the
synced HEAD (keeps untracked files, does not touch submodules with the pin) and `git stash pop` re-applies the
user's edit - but since the edit genuinely overlaps upstream's change, the pop conflicts again and keeps the
stash; the user must resolve `shared.txt` (or the app offers "keep upstream" = `reset --hard` + `stash drop`,
"keep mine" = `checkout --theirs -- shared.txt`, etc.). `stash show -p` shows exactly what was saved.

---

## Q6 - Two own commits moving A's gitlink (C1: A->S1, C2: A->S2; upstream A->U)

Setup: S1=6373153 (in "seal work"), S2=64c5552 "A: local work 2" sealed by a new super commit "seal work 2"
(1023ffa) on top of "local unrelated". U=5e5f8ab. A's uncommitted edit moved to `A/local.txt`.

```
$ git -C local $PIN rebase --merge --autostash refs/remotes/origin/main      (FIRST stop; hints elided)
Created autostash: 91c5131
Rebasing (1/3)Failed to merge submodule A
CONFLICT (submodule): Merge conflict in A
error: could not apply d24b9e2... seal work
Could not apply d24b9e2... # seal work
$ git -C local ls-files -u
160000 94d3a4871f234ef7b6df3af4c60060dd042700e9 1	A
160000 5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795 2	A
160000 6373153df4f7d6f74209dd07e2d7af908919cdca 3	A       (= S1)
$ git -C local/A rev-parse HEAD      -> 64c5552... (= S2, NOT stage 3: A's branch is already past S1)
$ git -C local/A $PIN rebase --merge --autostash 5e5f8ab...
Created autostash: b8744a6
Rebasing (1/2)Rebasing (2/2)Applied autostash.
Successfully rebased and updated refs/heads/main.
$ git -C local/A log --oneline -5
9e4850f A: local work 2        (S2')
85805bf A: local work          (S1')
5e5f8ab A: upstream 2
99abee6 A: upstream 1
94d3a48 A: initial
$ git -C local $PIN add -- A ; git -C local $PIN rebase --continue
[detached HEAD b3e3609] seal work                       <= C1' now records S2' (whole branch), not S1'
 1 file changed, 1 insertion(+), 1 deletion(-)
Rebasing (2/3)Rebasing (3/3)Failed to merge submodule A (commits don't follow merge-base)
CONFLICT (submodule): Merge conflict in A
error: could not apply 1023ffa... seal work 2
Could not apply 1023ffa... # seal work 2
exit=1
SECOND stop:
$ git -C local ls-files -u
160000 6373153df4f7d6f74209dd07e2d7af908919cdca 1	A       (base  = S1)
160000 9e4850fc2564dc7d2c2a01433f2831fed4599075 2	A       (ours  = S2', what C1' recorded)
160000 64c5552c79392de058b93e1eb6f349aecf4bf4a9 3	A       (theirs= S2, the original)
u UU S.MU 160000 160000 160000 160000 6373153... 9e4850f... 64c5552... A
done: pick d24b9e2 # seal work / pick c1a2b58 # local unrelated / pick 1023ffa # seal work 2
$ git -C local/A rev-parse HEAD                              -> 9e4850f... (S2')
$ git -C local/A merge-base --is-ancestor 64c5552 HEAD       -> exit 1   (A HEAD does NOT contain stage 3)
$ git -C local/A merge-base --is-ancestor HEAD 64c5552       -> exit 1
resolve: keep A at S2' -> $ git -C local $PIN add -- A ; rebase --continue
Applied autostash.
Successfully rebased and updated refs/heads/main.
exit=0                                                       <= no "now empty" message, no --skip needed
$ git -C local log --oneline '@{u}..HEAD'
f730b48 local unrelated
b3e3609 seal work                                            <= "seal work 2" is GONE (dropped as empty)
HEAD:A = A HEAD = 9e4850f ; submodule status: " 9e4850f A (heads/main) / +37c40e1 B (heads/main)"
```

Q6b - alternative first-stop resolution that preserves both commits: rebase A's branch exactly as above,
but record only S1' (the rewritten stage 3) in C1' with `update-index`, leaving A's checkout at S2':

```
S1'=4ceb6bf S2'=ab12e8a (this run's rewritten SHAs)
$ git -C local update-index --cacheinfo 160000,4ceb6bf943914c06e7df494f149b009ce2eaf7b2,A   -> exit 0
$ git -C local ls-files -s -- A   -> 160000 4ceb6bf... 0	A       ; ls-files -u -> (empty)
$ git -C local $PIN rebase --continue
[detached HEAD 5258536] seal work
Rebasing (2/3)Rebasing (3/3)Failed to merge submodule A (commits don't follow merge-base)
CONFLICT (submodule): Merge conflict in A
error: could not apply 3abf109... seal work 2
exit=1
$ git -C local ls-files -u
160000 6373153... 1	A     (base S1)
160000 4ceb6bf... 2	A     (ours S1')
160000 c75fd7f... 3	A     (theirs S2)
A HEAD (ab12e8a = S2') contains stage 3 (S2)? exit 1  (no - it is the REWRITTEN S2)
$ git -C local $PIN add -- A ; rebase --continue
[detached HEAD 94bd3c7] seal work 2
 1 file changed, 1 insertion(+), 1 deletion(-)
Applied autostash.
Successfully rebased and updated refs/heads/main.
final:
94bd3c7  seal work 2      A=ab12e8a... (S2')
1bdb3d8  local unrelated  A=4ceb6bf... (S1')
5258536  seal work        A=4ceb6bf... (S1')
```

Conclusion: with more than one own commit moving the same gitlink, "rebase A during the FIRST stop" rewrites
A's whole branch, so A's HEAD is S2' (not S1') already at the first stop, and A HEAD != stage 3 at BOTH stops
(first: HEAD=S2 vs stage3=S1; second: HEAD=S2' vs stage3=S2). Git always stops again on C2 ("commits don't
follow merge-base") because S2' does not contain the original S2. Resolving both stops with `git add -- A`
makes C1' record S2' and then C2' becomes empty and is DROPPED SILENTLY - the second commit vanishes from
history. To preserve C1/C2 one-to-one, record the rewritten equivalent of each stop's stage 3 via
`git update-index --cacheinfo 160000,<Sn'>,A` (compute Sn' from A's rewritten branch by offset:
`git -C A rev-parse HEAD~k` with k = `git -C A rev-list --count <stage3>..<A HEAD before rebase>`), and keep the
plain `git add -- A` only for the LAST stop. Either way the `head == stage3` check must be relaxed to
"A HEAD (or its rewritten ancestor) corresponds to stage 3" once a rebase of A has already been done in this run.

---

## Q7 - Detached A with a commit made while detached

Fixture variant (own copy `local7`): A detached at S1=6373153, A's `main` left at the clone commit 94d3a48
(A's uncommitted edit in `A/local.txt`; A/untracked-A.txt present).

```
$ git -C local7/A status -sb | head -1     -> ## HEAD (no branch)
$ git -C local7/A log --oneline --all --decorate -4
6373153 (HEAD) A: local work
94d3a48 (origin/main, origin/HEAD, main) A: initial
$ git -C local7 submodule status
 6373153df4f7d6f74209dd07e2d7af908919cdca A (heads/main-1-g6373153)     <= describe-style, not a branch name
 37c40e1ee5191cba65cfe674834a9dd434ca967f B (heads/main)
$ git -C local7/A rev-parse --abbrev-ref '@{u}'                -> fatal: HEAD does not point to a branch (128)
$ git -C local7/A rev-list --left-right --count 'HEAD...@{u}'  -> fatal: HEAD does not point to a branch (128)
$ git -C local7/A symbolic-ref refs/remotes/origin/HEAD        -> refs/remotes/origin/main (exit 0)
$ git -C local7/A rev-list --left-right --count 'HEAD...refs/remotes/origin/HEAD'   -> 1	0   (works while detached)
```

(i) `switch -c work`, then rebase during the superproject stop:

```
$ git -C local7/A $PIN switch -c work                          -> Switched to a new branch 'work'
(superproject rebase stops on A as in Q3; stage2=5e5f8ab stage3=6373153 = A HEAD)
$ git -C local7/A $PIN rebase --merge --autostash 5e5f8ab...
Created autostash: 70343b4
Rebasing (1/1)Applied autostash.
Successfully rebased and updated refs/heads/work.
3cf903a (HEAD -> work) A: local work
5e5f8ab (origin/main, origin/HEAD) A: upstream 2
99abee6 A: upstream 1
94d3a48 (main) A: initial
(git add -- A ; rebase --continue -> Successfully rebased and updated refs/heads/main.)
result: HEAD ref: refs/heads/work
$ git -C local7/A rev-parse --abbrev-ref '@{u}'               -> fatal: no upstream configured for branch 'work' (128)
$ git -C local7/A rev-list --left-right --count 'HEAD...@{u}' -> fatal: no upstream configured for branch 'work' (128)
$ git -C local7/A rev-list --left-right --count 'HEAD...origin/main' -> 1	0
$ git -C local7 submodule status
 3cf903aec3e18fe92a40eb2808410b788508e9f8 A (heads/work)
+37c40e1ee5191cba65cfe674834a9dd434ca967f B (heads/main)
A's stale 'main': 94d3a48 A: initial
(iv) $ git -C local7/A branch --set-upstream-to=origin/main work -> "branch 'work' set up to track 'origin/main'." ; @{u} then works
```

(ii) rebase while detached, then `branch -f main HEAD && switch main`:

```
$ git -C local7/A $PIN rebase --merge --autostash 5e5f8ab...      (detached)
Created autostash: 35e6876
Rebasing (1/1)Applied autostash.
Successfully rebased and updated detached HEAD.
## HEAD (no branch)
3cf903a (HEAD) A: local work
94d3a48 (main) A: initial ...
$ git -C local7/A branch -f main HEAD                              -> exit 0
$ git -C local7/A $PIN switch main
Switched to branch 'main'
M	local.txt
Your branch is ahead of 'origin/main' by 1 commit.
## main...origin/main [ahead 1]
result: HEAD ref refs/heads/main ; upstream origin/main ; counts 1/0
 3cf903aec3e18fe92a40eb2808410b788508e9f8 A (heads/main)
$ git -C local7/A merge-base --is-ancestor 94d3a48 HEAD -> exit 0   (old main is an ancestor: branch -f lost nothing)
```

(iii) one step before rebasing: `switch -C main` while detached:

```
$ git -C local7/A $PIN switch -C main
Switched to and reset branch 'main'
Your branch and 'origin/main' have diverged, and have 1 and 2 different commits each, respectively.
## main...origin/main [ahead 1, behind 2]      (uncommitted edit and untracked file kept; upstream = origin/main)
```

`refs/remotes/origin/HEAD` existence:

```
after git clone --recurse-submodules (local.pristine/A):    refs/remotes/origin/main   (exit 0)
after git clone (no recurse) + git submodule update --init -- A (local7b/A):   refs/remotes/origin/main   (exit 0)
   local7b/A: "* (HEAD detached at 5e5f8ab) / main / remotes/origin/HEAD -> origin/main / remotes/origin/main"
   local7b .git/config: submodule.A.update=merge ; submodule.A.branch UNSET ; .gitmodules submodule.A.branch=main
after git submodule update --remote -- A:  still refs/remotes/origin/main ; A still detached
$ git -C local7b/A ls-remote --symref origin HEAD   -> ref: refs/heads/main	HEAD
```

Conclusion: `switch -c work` (i) yields a branch with NO upstream - every `@{u}`-based assessment fails, `submodule
status` shows `(heads/work)`, and A's real `main` is left stale. Rebasing detached and then `branch -f main HEAD &&
switch main` (ii) - or `switch -C main` first (iii) - ends on `main` tracking `origin/main` with correct counts.
Guard (ii)/(iii) with `git -C A merge-base --is-ancestor main HEAD` (refuse if exit != 0: `main` has commits the
detached HEAD lacks). `refs/remotes/origin/HEAD` exists after both clone paths and after `submodule update`, so
`HEAD...refs/remotes/origin/HEAD` is the robust detached-state comparison; `.gitmodules` `branch` is not copied to
`.git/config` and must be read with `git config -f .gitmodules`.

---

## Q8 - File conflict in the superproject (local commit also edits `shared.txt` line 2)

Setup: extra local commit 9ba31ff "local: edit shared line 2" on top of c1a2b58; README edit kept uncommitted;
A's uncommitted edit in `A/local.txt`. Capture: ` M A / M README.md / ?? untracked-super.txt`. HEAD=9ba31ff.

```
$ git -C local $PIN rebase --merge --autostash refs/remotes/origin/main     -> stop 1 on A (as Q3); resolved as Q3
$ git -C local $PIN rebase --continue
[detached HEAD 06300e2] seal work
 1 file changed, 1 insertion(+), 1 deletion(-)
Rebasing (2/3)Rebasing (3/3)Auto-merging shared.txt
CONFLICT (content): Merge conflict in shared.txt
error: could not apply 9ba31ff... local: edit shared line 2
Could not apply 9ba31ff... # local: edit shared line 2
exit=1
$ git -C local status --porcelain=v2 | grep '^u'
u UU N... 100644 100644 100644 100644 e21619f... af88be3... 0da1035... shared.txt      (mode 100644 vs gitlink's 160000)
$ git -C local ls-files -u
100644 e21619fa2d87272ee453301c44cf080ab10e7c72 1	shared.txt
100644 af88be3ce7f43a5622085e93e9539d2253cf3de9 2	shared.txt
100644 0da1035f5f59f0087e4e10bde9ca7da8d25a98a2 3	shared.txt
$ git -C local diff --name-only --diff-filter=U    -> shared.txt
shared.txt:
shared 1
<<<<<<< HEAD
shared 2 (upstream)
=======
shared 2 (local commit)
>>>>>>> 9ba31ff (local: edit shared line 2)
shared 3
done: pick d24b9e2 # seal work / pick c1a2b58 # local unrelated / pick 9ba31ff # local: edit shared line 2
stopped-sha: 9ba31ffca37baf97226deaf27e06fb7b8e3853d2

$ git -C local $PIN rebase --abort          -> Applied autostash. exit 0
HEAD == pre-rebase (9ba31ff): YES
status --porcelain: " M A / M README.md / ?? untracked-super.txt"  IDENTICAL to capture
stash list: (empty) ; untracked-super.txt present ; README.md = "# super / local README edit"
BUT A (rebased during stop 1) is NOT undone by the superproject abort:
$ git -C local/A log --oneline -3   -> bdca166 A: local work / 5e5f8ab A: upstream 2 / 99abee6 A: upstream 1
$ git -C local rev-parse HEAD:A     -> 6373153...        => A HEAD != recorded (an A1 state, cf. Q4)
1 .M SCMU 160000 160000 160000 6373153... 6373153... A

Re-run:
$ git -C local $PIN rebase --merge --autostash refs/remotes/origin/main   -> stop on A again, same stages
   94d3a48 / 5e5f8ab / 6373153 ; A HEAD = bdca166 (contains stage 2; is the rewritten stage 3)
   -> resolved with plain `git add -- A` (no second A rebase), continue -> stop on shared.txt as above
$ git -C local $PIN checkout --ours -- shared.txt     ("ours" = upstream side during a rebase)
shared 1 / shared 2 (upstream) / shared 3
$ git -C local $PIN add shared.txt
$ git -C local $PIN rebase --continue
Applied autostash.
Successfully rebased and updated refs/heads/main.
exit=0                                        <= NO "The previous cherry-pick is now empty" ; rebase-merge gone ; --skip NOT needed
$ git -C local log --oneline '@{u}..HEAD'    -> 04d082a local unrelated / 06300e2 seal work   (the emptied commit was dropped)
```

Q8b - does `--empty=` change that?

```
--empty=drop : rebase --continue -> Applied autostash. Successfully rebased ... ; log: local unrelated / seal work
--empty=keep : identical (commit still dropped)
--empty=stop : identical (commit still dropped)
```

Conclusion: a file conflict is distinguishable from a gitlink stop by the mode in `ls-files -u` / the `u` line
(100644 vs 160000) and by `diff --name-only --diff-filter=U`. `rebase --abort` fully restores the superproject
(HEAD SHA, identical status, autostash re-applied, untracked kept, stash list empty) but does NOT roll back a
submodule rebase already performed during an earlier stop - the app must record each submodule's pre-rebase
HEAD and restore it itself on abort (or treat the new A1 state as "A already rebased; resolve with `git add`" on
re-run, which works). CONTRADICTION with the design assumption: in git 2.50.1 with the merge backend, resolving
a conflict to upstream's content makes the pick empty and `rebase --continue` DROPS it silently with exit 0 -
there is no "now empty" message and `rebase --skip` is never needed (regardless of `--empty=`).

---

## Q9 - Config hazards

(a) `rebase.updateRefs` with a backup branch at HEAD:

```
$ git config --global rebase.updateRefs true
$ git -C local branch gitswitch-before-sync-x HEAD        -> c1a2b584fbb46c49317cd980b10fda07fa0c4253
$ git -C local rebase --merge --autostash refs/remotes/origin/main        (NO pin) -> stop on A
$ cat .git/rebase-merge/update-refs
refs/heads/gitswitch-before-sync-x
c1a2b584fbb46c49317cd980b10fda07fa0c4253
0000000000000000000000000000000000000000
(A resolved as Q3; rebase --continue)
Updated the following refs with --update-refs:
	refs/heads/gitswitch-before-sync-x
$ git -C local rev-parse gitswitch-before-sync-x         -> 7bd76442... = new HEAD
7bd7644 (HEAD -> main, gitswitch-before-sync-x) local unrelated
60ffc9b seal work
c0e382a (origin/main, origin/HEAD) bump B
RESULT: backup branch MOVED to the new HEAD (backup destroyed)

repeat WITH -c rebase.updateRefs=false (global still true):
rebase-merge/update-refs file: (none)
$ git -C local rev-parse gitswitch-before-sync-x         -> c1a2b584... (unchanged)
RESULT: backup branch did NOT move (pin works)
$ git config --global --unset rebase.updateRefs
```

(b) `submodule.recurse true`:

```
$ git config --global submodule.recurse true
before: A HEAD=6373153 refs/heads/main
$ git -C local rebase --merge --autostash refs/remotes/origin/main        (NO pin)
Created autostash: eabaeaa
Rebasing (1/2)hint: ...
Failed to merge submodule A
CONFLICT (submodule): Merge conflict in A
error: could not apply d24b9e2... seal work
Could not apply d24b9e2... # seal work
exit=1
after: A HEAD=6373153 refs/heads/main (unchanged) ; A status " M local.txt / ?? untracked-A.txt" ; B HEAD=37c40e1 refs/heads/main (unchanged)
super: HEAD c0e382a ; stages 94d3a48/5e5f8ab/6373153 ; "1 .M SC.. ... B" ; rebase in progress: yes ; stash list empty
$ git -C local rebase --abort (NO pin)   -> Applied autostash. exit 0 ; A and B still unchanged
WITH the pin (global still true): identical - A HEAD 6373153 main, B 37c40e1 main, same stop
$ git config --global --unset submodule.recurse
```

(b) supplement - the SAME config on the Q5 recovery step (`reset --hard`), throwaway copy whose B worktree lags
the checked-out gitlink and whose A has NOT fetched upstream's commits:

```
$ git -C copy -c submodule.recurse=true reset --hard
fatal: failed to unpack tree object 5e5f8abfa2f4d7e6c4bbcc46b09c2af293573795
error: Submodule 'A' could not be updated.
fatal: Could not reset index file to revision 'HEAD'.
exit=128
B HEAD after:  c391774 DETACHED        <= B was moved off `main` and detached
A HEAD after:  6373153 refs/heads/main (A's dirty state kept, but the super index reset failed half-way)
$ git -C copy $PIN reset --hard         -> HEAD is now at c0e382a bump B ; B still 37c40e1 refs/heads/main
```

Conclusion: `rebase.updateRefs=true` (a popular global setting) silently drags any branch pointing at a rebased
commit - including the app's own backup branch - onto the rewritten history; `-c rebase.updateRefs=false` is
mandatory whenever a backup ref is used. `submodule.recurse=true` did NOT change the behaviour of `git rebase`
itself in 2.50.1 (submodule worktrees untouched with or without the pin), but it DOES change `reset --hard` /
`checkout`: it detaches submodules onto the gitlink and aborts half-way when the submodule lacks the object. Keep
`-c submodule.recurse=false` on every superproject command, not just the rebase.

---

## Q10 - `git stash push` with A dirty and ahead

(a) fixture as-is (A HEAD == recorded gitlink; A dirty; A ahead of origin/main by 1):

```
$ git -C local $PIN stash push          -> Saved working directory and index state WIP on main: c1a2b58 local unrelated
$ git -C local stash show --stat
 README.md | 1 +
 1 file changed, 1 insertion(+)
(--include-untracked: same)
submodule status before == after: " 6373153... A (heads/main) / 37c40e1... B (heads/main)"
super status after: " M A / ?? untracked-super.txt"      (A's dirty content is still visible; only README was stashed)
A HEAD unchanged (6373153); A status " M A.txt / ?? untracked-A.txt"; A/untracked-A.txt present
$ git -C local $PIN stash pop           -> ... Dropped refs/stash@{0} ; exit 0 ; super status == capture
```

(b) A HEAD != recorded (extra unsealed commit 9669cff in A):

```
before: submodule status "+9669cff... A (heads/main)" ; "1 .M SCMU 160000 160000 160000 6373153... 6373153... A"
$ git -C local $PIN stash push          -> Saved working directory and index state WIP on main: c1a2b58 local unrelated
$ git -C local stash show --stat        ->  README.md | 1 +      (A NOT listed)
$ git -C local rev-parse 'stash@{0}:A' HEAD:A   -> 6373153... / 6373153...   (stash tree's gitlink == HEAD's: NOT carried)
submodule status after: still "+9669cff... A (heads/main)" ; super status " M A / ?? untracked-super.txt"
A HEAD unchanged (9669cff) ; A status untouched ; untracked-A.txt present
$ git -C local $PIN stash pop           -> exit 0 ; "modified: A (new commits, modified content, untracked content)"
```

(c) with the gitlink STAGED (`git add A` first, status "MM A"):

```
$ git -C local $PIN stash push ; stash show --stat
 A         | 2 +-
 README.md | 1 +
after push: super status " M A" ; submodule status still "+9669cff... A" ; A HEAD still 9669cff
   (the stash reset the INDEX gitlink to HEAD:A but did NOT move A's checkout)
$ git -C local $PIN stash pop           -> exit 0 ; A staged again
```

Conclusion: `git stash push` in the superproject never touches a submodule's worktree, HEAD or untracked files. An
unstaged gitlink difference (A checked out at a commit != HEAD:A) is NOT carried by the stash at all; a STAGED
gitlink is carried as an index change only. The submodule's own dirty state must be stashed inside the submodule
(which is what `--autostash` on the submodule rebase does).

---

## Q11 - Bundles

```
$ time git -C local bundle create "$SB/b/super.bundle" --all      -> 0.01s user 0.01s system 0.031 total
$ git bundle verify "$SB/b/super.bundle"
/.../b/super.bundle is okay
The bundle contains these 4 refs:
c1a2b584fbb46c49317cd980b10fda07fa0c4253 refs/heads/main
b726cbea53b20e17c39c69949ac5f261ba712129 refs/remotes/origin/HEAD
b726cbea53b20e17c39c69949ac5f261ba712129 refs/remotes/origin/main
c1a2b584fbb46c49317cd980b10fda07fa0c4253 HEAD
The bundle records a complete history.
The bundle uses this hash algorithm: sha1
$ du -h super.bundle          -> 4.0K       (12 loose objects in super; A has 6 of its own)
$ git clone -q "$SB/b/super.bundle" "$SB/b/restore/super"         -> exit 0 ; log: c1a2b58 / d24b9e2 / b726cbe ; origin = the bundle file
$ git -C restore/super rev-parse HEAD:A                            -> 6373153df4f7d6f74209dd07e2d7af908919cdca
$ git -C restore/super cat-file -t 6373153...                      -> fatal: git cat-file: could not get object info (exit 128)
$ git -C restore/super submodule update    (no --init)            -> exit 0, does nothing
$ ls -la restore/super/A                                           -> empty directory
$ git -C restore/super submodule status
-6373153df4f7d6f74209dd07e2d7af908919cdca A
-37c40e1ee5191cba65cfe674834a9dd434ca967f B
$ git -C restore/super -c submodule.A.url=/nonexistent submodule update --init -- A
fatal: repository '/nonexistent' does not exist
fatal: clone of '/nonexistent' into submodule path '.../restore/super/A' failed
Failed to clone 'A' a second time, aborting                        (exit 1)
$ git -C local/A bundle create "$SB/b/A.bundle" --all ; same for B  -> 4.0K each
$ git bundle list-heads A.bundle
6373153df4f7d6f74209dd07e2d7af908919cdca refs/heads/main            (local branch WITH the unpushed commit)
94d3a4871f234ef7b6df3af4c60060dd042700e9 refs/remotes/origin/HEAD
94d3a4871f234ef7b6df3af4c60060dd042700e9 refs/remotes/origin/main
6373153df4f7d6f74209dd07e2d7af908919cdca HEAD
$ git -C restore/super -c submodule.A.url="$SB/b/A.bundle" submodule update --init -- A
Cloning into '.../restore/super/A'...
Submodule path 'A': checked out '6373153df4f7d6f74209dd07e2d7af908919cdca'
$ cat restore/super/A/.git   -> gitdir: ../.git/modules/A
```

Conclusion: a superproject bundle is cheap and complete for the superproject's own history but contains only
the gitlink SHAs, none of the submodule objects - restoring it leaves empty submodule directories and
`submodule status` with `-` prefixes; `--init` would re-clone from the `.gitmodules` URL (network), not from
the bundle. A full local backup = one `bundle create --all` per repository (super + each submodule); a
submodule bundle can be fed to `submodule update --init` via `-c submodule.<name>.url=<bundle>`. Note
`--all` excludes `refs/stash` and the rebase autostash.

---

## Q12 - Alignment tools for B (from a post-Q3 state: super synced, B still at 37c40e1 on `main`, HEAD:B = c391774)

```
$ git -C local config --get submodule.B.update            -> merge   (exit 0: copied from .gitmodules at clone)
$ git -C local config -f .gitmodules --get submodule.B.update -> merge

(1) $ git -C local $PIN submodule update --merge -- B
Updating 37c40e1..c391774
Fast-forward
 B.txt | 1 +
Submodule path 'B': merged in 'c391774105c5fe2ee8ae55af6518239be4ad20f8'     -> B on refs/heads/main @ c391774
(2) $ git -C local $PIN submodule update --rebase -- B
Successfully rebased and updated refs/heads/main.
Submodule path 'B': rebased into 'c391774105c5fe2ee8ae55af6518239be4ad20f8'  -> B on refs/heads/main @ c391774
(3) $ git -C local $PIN submodule update -- B         (plain; .git/config says merge)
Updating 37c40e1..c391774 / Fast-forward / Submodule path 'B': merged in ...   -> B on refs/heads/main (NOT detached)
(3b) $ git -C local -c submodule.B.update=checkout submodule update -- B
Submodule path 'B': checked out 'c391774105c5fe2ee8ae55af6518239be4ad20f8'   -> B DETACHED ; submodule status: " c391774... B (remotes/origin/HEAD)"
(3c) key removed from .git/config, plain update -> still "merged in" (falls back to .gitmodules' update=merge) -> on main
(4) $ git -C local/B $PIN checkout --detach c391774...
HEAD is now at c391774 B: upstream 1                                          -> B DETACHED
(5) $ git -C local/B $PIN merge --ff-only --no-edit --no-stat -- c391774...  (Q3's choice)
Updating 37c40e1..c391774
Fast-forward                                                                  -> B on refs/heads/main
(6) B has a LOCAL commit (non-ff): $ git -C local $PIN submodule update --merge -- B
Merge made by the 'ort' strategy.                                             -> creates a MERGE COMMIT c385ed1 on B's main
*   c385ed1 Merge commit 'c391774105c5fe2ee8ae55af6518239be4ad20f8'
|\
| * c391774 B: upstream 1
* | 9ff582e B: local
|/
* 37c40e1 B: initial
(6b) same non-ff case: $ git -C local/B $PIN merge --ff-only ... -> fatal: Not possible to fast-forward, aborting. (exit 128)
(7) B DETACHED + $ git -C local $PIN submodule update --merge -- B -> "merged in" but B stays DETACHED
```

Conclusion: `submodule update --merge`, `--rebase`, and plain `update` (because `update = merge` was copied
into `.git/config` at clone and is also read from `.gitmodules` as a fallback) all leave B on `main`; only
`update` with `update=checkout` and `checkout --detach` detach B. But `submodule update --merge` silently
creates a merge commit when B has its own commits, and none of the `submodule update` modes re-attach a
detached submodule. The explicit `merge --ff-only -- <recorded>` inside B is the predictable choice: it
fast-forwards when B is merely behind and refuses (exit 128) otherwise, so the app can route the non-ff case
to the Q3 rebase path instead.

---

## Q13 (extra) - Base choice when A's remote tip != the gitlink upstream recorded (target)

One more commit (4eb0ead "A: upstream 3") pushed to `A.git` without a superproject bump.

```
$ git -C local rev-parse HEAD:A origin/main:A   -> 6373153 (recorded) / 5e5f8ab (target)
$ git -C local/A rev-parse HEAD origin/main     -> 6373153 (head)     / 4eb0ead (tip)
target is ancestor of tip: exit 0
(a) rebase A onto TARGET (stage 2 = 5e5f8ab):
   A after: 65b470e A: local work ; rev-list HEAD...@{u} = 1/1 (ahead 1, behind 1) ; HEAD:A == A HEAD
   super diff origin/main..HEAD -- A: -Subproject 5e5f8ab  +Subproject 65b470e
(b) rebase A onto TIP (A's origin/main = 4eb0ead):
   A after: 7d65141 A: local work ; rev-list HEAD...@{u} = 1/0
   super diff origin/main..HEAD -- A: -Subproject 5e5f8ab  +Subproject 7d65141   (the rewritten "seal work" now also bumps A past upstream's target)
git's gitlink merge accepted both, since both A HEADs contain stage 2.
```

Conclusion: rebase onto the TARGET (stage 2 = what the superproject's upstream recorded), not the submodule's
remote tip. Rebasing onto the tip silently turns the user's "seal work" into an unreviewed "bump A" in the
superproject. After a target-based rebase the submodule may legitimately read "behind" its own remote branch;
report that as informational, not as a failed sync.

---

## Recommended algorithm

Notation: `S` = superproject path, `P` = each submodule path (from `git -C S submodule--helper list` or
`git -C S config -f .gitmodules --get-regexp '^submodule\..*\.path$'`), `UP` = `refs/remotes/origin/main`
(the superproject branch's upstream; use `git -C S rev-parse --symbolic-full-name @{u}`).
Every command carries `$PIN` (as an argv array) and the env
`GIT_EDITOR=true GIT_SEQUENCE_EDITOR=true LC_ALL=C GIT_TERMINAL_PROMPT=0`.

### 0. Fetch (no side effects on worktrees)

```
git -C S   $PIN fetch --quiet --no-recurse-submodules -- origin
git -C S/P $PIN fetch --quiet -- origin                          # each P
```

### 1. Assess

```
git -C S rev-list --left-right --count HEAD...@{u}              # ahead<TAB>behind
git -C S log --oneline @{u}..HEAD                               # own commits
git -C S status --porcelain=v2                                  # dirty? (autostash needed) ; 'S' lines = submodule state
per P:
  recorded=$(git -C S rev-parse HEAD:P)   target=$(git -C S rev-parse UP:P)
  head=$(git -C S/P rev-parse HEAD)
  branch=$(git -C S/P symbolic-ref -q HEAD)          # empty => detached (Q7)
  tipref=refs/remotes/origin/HEAD (exists after clone and submodule update; Q7) or origin/$(git -C S config -f .gitmodules --get submodule.<name>.branch)
  git -C S/P rev-list --left-right --count HEAD...$tipref      # never @{u} while detached / on a fresh branch (Q7)
  git -C S/P status --porcelain                                 # dirty => submodule autostash needed (Q3 hazard)
  [ "$head" = "$recorded" ]  || STOP: A1 state (git will not catch it, Q4/Q8) - offer "adopt: git -C S add -- P + commit" or "reset P to recorded"
  git -C S/P merge-base --is-ancestor $head $target  -> 0 : P is behind or equal; will be aligned in step 5 (ff)
  git -C S/P merge-base --is-ancestor $target $head  -> 0 : P already contains target; gitlink pick may still stop if commits' gitlinks differ
  both 1                                             : diverged; expect a gitlink stop for P (Q3)
  git -C S/P merge-base --is-ancestor $target $tipref -> 1 : warn "upstream superproject records an A commit not on A's branch" (Q13)
```

Ancestry must always be asked inside `S/P` - the superproject store has no submodule objects (Q1).

### 2. Backup (optional but recommended)

```
git -C S   branch gitswitch-before-sync-<id> HEAD
git -C S/P branch gitswitch-before-sync-<id> HEAD     # each P that will be rebased; ALSO remember $head per P for abort (Q8)
# rebase.updateRefs=false in $PIN is what keeps these from moving (Q9a)
# optional file backup: git -C S bundle create ... --all  AND  one bundle per P (Q11)
```

### 3. Rebase the superproject

```
git -C S $PIN rebase --merge --autostash UP
```

exit 0 -> go to step 4 checks. exit != 0 -> detect the stop:

```
test -d "$(git -C S rev-parse --path-format=absolute --git-path rebase-merge)"   # rebase in progress
git -C S ls-files -u -z                                                             # unmerged entries
git -C S status --porcelain=v2 -z                                                   # 'u' lines
git -C S diff --name-only --diff-filter=U
```

Per unmerged path:
- mode `160000` -> gitlink stop (Q3): stages s1 (base), s2 (upstream), s3 (own).
  ```
  # precondition (Q3/Q6): A HEAD must be s3 - or, if P was already rebased in this run, the rewritten equivalent of s3
  git -C S/P $PIN rebase --merge --autostash <s2>          # rebase onto TARGET (stage 2), not the tip (Q13)
  git -C S/P ls-files -u ; git -C S/P stash list             # BOTH must be empty (Q3: autostash conflict => UU + stash@{0}: autostash, exit still 0)
  git -C S   $PIN add -- P                                  # single own commit moving P (or the LAST such commit)
  # OR, when more own commits move P later in the todo (Q6b): git -C S update-index --cacheinfo 160000,<rewritten s3>,P
  git -C S   $PIN rebase --continue
  ```
  Detached P (Q7): rebase detached, then `git -C S/P merge-base --is-ancestor <branch> HEAD && git -C S/P branch -f <branch> HEAD && git -C S/P $PIN switch <branch>`
  (or `switch -C <branch>` first). Do NOT `switch -c work`.
- mode `100644` (or any blob) -> file conflict (Q8): surface to the user, or `git -C S $PIN rebase --abort`
  and then restore every P you already rebased (`git -C S/P $PIN switch -C <branch> <saved head>` - the superproject
  abort does not do this, Q8). Resolving to upstream's version makes the pick empty and `--continue` drops it
  silently (Q8/Q8b) - no `--skip` handling is needed, but tell the user the commit vanished.

Loop until `rebase-merge` no longer exists.

### 4. Post-rebase checks (the rebase can "succeed" and still leave a mess)

```
git -C S ls-files -u                  # non-empty after exit 0 => autostash pop conflict (Q5)
git -C S stash list                   # "stash@{0}: autostash" => same; offer: reset --hard (pinned) + stash pop / drop / checkout --theirs
git -C S/P ls-files -u ; git -C S/P stash list          # same for each rebased P (Q3)
```

### 5. Align lagging submodules

```
recorded=$(git -C S rev-parse HEAD:P) ; head=$(git -C S/P rev-parse HEAD)
[ "$head" = "$recorded" ] && skip
git -C S/P merge-base --is-ancestor HEAD $recorded && git -C S/P $PIN merge --ff-only --no-edit --no-stat -- $recorded   # stays on branch (Q3, Q12)
# non-ff (P has own commits not sealed): do NOT use `submodule update --merge` (creates a merge commit, Q12-6); route to the rebase path
# detached P: `checkout --detach $recorded` keeps it detached; re-attach per Q7 first if the user wants a branch
```

### 6. Verify

```
git -C S rev-list --left-right --count HEAD...@{u}     # behind must be 0
git -C S log --oneline @{u}..HEAD                      # own commits, original order
git -C S submodule status                              # no '+' / '-' prefixes
git -C S status --porcelain                            # equals the pre-sync capture (staged changes come back unstaged)
git -C S stash list ; git -C S ls-files -u ; same for each P
```

### Contradictions / corrections to the design assumptions

1. **Rebase-during-stop**: confirmed viable (Q3), but (a) `git add -- P` records P's HEAD even if P's worktree is in a
   conflicted state, and (b) the submodule's autostash can conflict while its rebase still exits 0 - check
   `ls-files -u` and `stash list` inside P before `git add`. (c) With two or more own commits moving the same
   gitlink, naive per-stop `git add -- P` collapses them and the later commit is silently dropped (Q6);
   use `update-index --cacheinfo` with the rewritten intermediate SHA (Q6b).
2. **`head == recorded` precondition**: git never enforces it (Q4, Q4b); the stop's stage 3 is the commit's gitlink,
   not P's HEAD. After a superproject `rebase --abort`, P stays rebased (Q8), so the precondition fails on the next
   run unless the app restores P or recognises "P HEAD contains stage 2 and is the rewritten stage 3 -> just `git add`".
3. **Base choice**: rebase P onto the TARGET (stage 2 = `UP:P`), not P's remote tip (Q13). Expect P to report
   "behind its remote" afterwards when upstream has not yet bumped it.
4. **`switch -c`**: don't. It leaves P on a branch with no upstream (`@{u}` fails, Q7-i) and a stale `main`.
   Rebase detached then `branch -f <branch> HEAD && switch <branch>` (guarded by `merge-base --is-ancestor`), or
   `switch -C <branch>` before the rebase.
5. **Pins**: `fetch.recurseSubmodules=false` is required (default fetch recurses on-demand, Q1). `rebase.updateRefs=false`
   is required with backup branches (Q9a). `submodule.recurse=false` is irrelevant to `git rebase` in 2.50.1 but is
   required for `reset --hard` / `checkout` (Q9b supplement). `rebase.backend=merge` gives the 3-stage gitlink stop and
   the `rebase-merge/` state directory this design relies on.
6. **"`rebase --continue` complains 'The previous cherry-pick is now empty'"**: does not happen in 2.50.1 - the emptied
   pick is dropped silently with exit 0, regardless of `--empty=` (Q8, Q8b). Design the UI around "commit dropped",
   not around handling `--skip`.
7. **Autostash**: during a stop it lives in `.git/rebase-merge/autostash`, NOT in `refs/stash` (`stash list` is empty,
   Q3); after a pop conflict the rebase reports success and the stash reappears as `stash@{0}: autostash` with no
   operation marker (Q5).
8. **Naive `pull --rebase --recurse-submodules`** refuses outright ("cannot rebase with locally recorded submodule
   modifications", Q2) - it cannot be the implementation.
9. **`git stash` in the superproject** never carries an unstaged gitlink change nor anything inside the submodule (Q10).
10. **`.gitmodules` `update = merge`** is copied to `.git/config` at clone and also used as a fallback, so plain
    `git submodule update` keeps B on `main` here - but `--merge` creates merge commits on non-ff and never re-attaches a
    detached submodule (Q12); prefer `merge --ff-only -- <recorded>` inside the submodule.
11. **Bundles**: a superproject bundle has no submodule objects (Q11); back up each submodule separately.
