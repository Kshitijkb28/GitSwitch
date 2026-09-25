# GitSwitch

Switch git identities **automatically, per folder**. Map each directory to a
GitHub account once — every repo inside commits and pushes as the right
identity, with the right SSH key, forever. No more wrong-account commits.

Built with **Tauri v2** (Rust) + **React** + **Tailwind**. Runs on macOS,
Windows, and Linux.

## How it works

GitSwitch manages a clearly-marked section of your `~/.gitconfig` using git's
native [`includeIf "gitdir:..."`](https://git-scm.com/docs/git-config#_conditional_includes)
mechanism:

- Each **profile** = a git identity (name + email) + an SSH key + a set of folders.
- Repos inside a profile's folders automatically use that profile's identity
  and SSH key (`core.sshCommand` with `IdentitiesOnly=yes`).
- The **default** profile covers everything else.
- A timestamped backup of `~/.gitconfig` is taken before every change (newest 5 kept).

Because each SSH key is registered to exactly one GitHub account, pushes
authenticate as the right account per folder — as long as remotes use SSH
(`git@github.com:...`). The app includes a one-click **"Convert repos to SSH"**
for folders that still use HTTPS.

## Features

- **Doctor** — one click finds the things that silently send commits to the
  wrong account: an `~/.ssh/config` block pinning one key for github.com, SSH
  keys not registered on any account, hand-written `includeIf` rules fighting
  the managed ones, HTTPS remotes (and **tokens embedded in remote URLs**),
  passphrase-protected keys, and missing folders. Most findings come with a
  one-click fix.
- **Profiles** — create/edit identities, assign folders with a native folder
  picker. A folder can belong to only one profile (auto-deduplicated).
- **Auto Assign** — scan a folder tree, check which of your GitHub accounts
  actually has access to each repo, and map them to the right profile.
- **Clone** — clone over SSH as the right account. The destination folder's
  profile decides the key, or pick one with **Clone as** (the new folder is then
  added to that profile). Downloads submodules with the same result as
  `git clone --recurse-submodules`, downloads Git LFS content (a clone without
  it looks complete but every large file is a pointer stub; the option is on
  by default and the result says what state the large files are in either
  way), notices when the repo is already cloned
  (and can switch an HTTPS clone to the SSH link), and explains refusals —
  unregistered key, missing org SSO authorization, or an organization that
  requires SSH certificates (`org-<id>@github.com` links). A **sparse** mode
  clones metadata only and lets you tick just the folders you need.
- **Repositories** — every repository your signed-in GitHub accounts can reach
  (owned, collaborator and organization), with search, filters and paging.
  Each one can be cloned in a click — straight into the Clone page with the
  right link and a suggested profile — or opened if it's already on disk.
  The first ten repositories appear as soon as GitHub answers; the rest load in
  the background (a "loading more" note shows until they are all in), and
  switching accounts clears the previous account's list at once.
- **Changes** — the everyday git loop, without leaving the app: see every
  changed file with its real diff, stage and unstage individual files, discard
  (with an explicit confirmation naming what gets deleted), commit as the
  folder's own identity, pull, push and update submodules. Pull offers all
  three strategies — fast-forward, merge, rebase — each with a sentence saying
  what it will do to *your* commits before it runs, because `git pull` alone
  behaves differently depending on settings you may not know you have. Nothing
  is forced and nothing is bypassed: no `--force`, no `--no-verify`, no
  amending a commit that is already pushed, and conflicts are surfaced for you
  to resolve rather than resolved behind your back. Long operations keep
  running while you browse other pages.
  - **Submodules** are listed from `git ls-files` plus `.gitmodules` — never
    `git submodule status`, which aborts on the first gitlink missing from
    `.gitmodules` and hides every other one. Each shows where it moved from and
    to, what is dirty inside it, and whether git can fetch it at all; clicking
    one opens it as its own repository.
  - **Sync — the latest from upstream, your commits on top.** For anyone who
    works on a branch that also receives other people's commits, in a
    repository with or without submodules. *Assess* fetches and shows exactly
    what would happen: how many of your commits are replayed on top of how many
    incoming ones, which submodules are rebased (onto the commit upstream
    records for them — never their remote's tip), which are fast-forwarded,
    which are left alone, and every reason the sync would refuse (a submodule
    checked out at a commit your branch does not record, two of your commits
    moving one submodule pointer, an upstream pointer nobody can fetch…). *Sync
    now* runs it: a backup branch in every repository it touches first, your
    uncommitted changes stashed and put back (or the sync refuses, if you turn
    that off), the superproject rebased with git's own behaviour pinned so
    `rebase.updateRefs` or `submodule.recurse` in your config cannot change the
    result, each submodule your commits move rebased when the superproject
    stops on its pointer, and the rest aligned afterwards. A conflict pauses it
    where the conflict is — resolve on the Changes page, then *Continue sync*;
    *Abort sync* puts the superproject **and** the rebased submodules back,
    which `git rebase --abort` alone would not. Commits upstream already
    contains are dropped and named; submodules checked out ahead of what the
    branch records are named, with one button to record them. Nothing is
    pushed — ever.
  - **Git LFS**: a clone made without git-lfs (or with `GIT_LFS_SKIP_SMUDGE`)
    looks complete but every large file is a pointer stub, and `git status`
    never says so. The Changes page counts the stubs and offers `git lfs pull`,
    setting the repository's LFS filters up first when they were never
    configured — the state in which `git lfs pull` exits 0 and downloads
    nothing. **Browse files…** opens the whole list folder by folder, with
    each file's real size shown before anything is downloaded, and downloads
    one file, one folder or any selection of them — `git lfs fetch --include`
    brings the objects, `git lfs checkout` writes them out. Both read their
    arguments as patterns, so each path is escaped to match only itself: a file
    really called `shot*1.bin` downloads that file and not its neighbours. A
    running download reports the file and the bytes it is on, and a failure
    names what is still missing rather than reporting success next time round.
  - **Inside submodules.** Every populated submodule gets its own section on
    the page — its conflicts, staged, unstaged and new files, with the same
    stage / unstage / discard / diff actions and its own commit box committing
    as the submodule's identity. Commit inside, then record the pointer in the
    superproject with one click. The parent's gitlink row jumps to the section.
  - **Branches.** Switch, create (from the current branch, another one, or the
    commit you are looking at), rename and delete from the page. A switch that
    would overwrite uncommitted changes is refused with the files named; a
    branch whose commits no other branch holds is only deleted after you
    confirm, and the result prints the command that brings it back.
  - **Tidy up.** *Undo last commit* (its changes come back staged; refused
    when the commit is already on the remote — revert it instead), *Stash
    changes* (with or without new files; never ignored ones), *Reset to
    upstream* (your unpushed commits leave the branch, a
    `gitswitch-before-reset-<time>` branch keeps them, and the undo command is
    printed), *Discard everything* (optionally deleting new files, optionally
    stashing first). Nothing that is already on the upstream is ever dropped:
    that would need a force-push, which GitSwitch never does.
  - **Stashes.** The list with apply, pop and drop (a dropped stash's id and
    the `git stash store` command that restores it are shown), each stash's
    files, and *Restore file* for taking one file back out of a stash — the
    recovery a failed autostash needs. A stash or autostash that cannot be
    re-applied leaves ordinary conflicts, and the page says where they came
    from.
  - **Conflicts.** *Keep mine* / *Take theirs* per file or for all, with both
    sides named in your words — during a rebase git's "ours" is the upstream
    and "theirs" is your commit, and the buttons map that for you.
  - **Pull with rebase on a dirty tree.** Rebase pulls still refuse a dirty
    tree by default; turn on *Stash my changes around the rebase* and git's
    autostash is used, with a re-apply that conflicts reported as such rather
    than hidden.
- **Push blocking** — per profile *and* per repository. A blocked repo refuses
  `git push` **in your terminal too**, via two mechanisms with different blind
  spots: a `pushInsteadOf` rewrite in the repo's own `.git/config` (which
  `--no-verify` cannot get past) and a `pre-push` hook (which catches remote
  forms the rewrite can't, and chains any hook already there — Git LFS's, for
  instance — instead of replacing it). Pull and fetch keep working. The app
  states plainly what this does *not* stop: it is a guard-rail against
  accidents, not a lock — anyone with a terminal can turn it off with one git
  command, a longer `pushInsteadOf` of their own wins (git picks the longest
  match, in any scope), and `GIT_CONFIG_NOSYSTEM=1` skips it.
- **Push lock** — the same block, written by a small privileged helper into
  **administrator-owned, system-wide** git config (`/etc/gitconfig` →
  `/etc/gitswitch/` on macOS and Linux), so that turning it off — from the app,
  a terminal, a script or an AI agent — needs the **administrator password**.
  Every lock and unlock shows the operating system's own prompt; the job is a
  one-time file whose checksum is part of the approved command, so a cached
  approval cannot be replayed with different content. A refused push prints an
  explanation addressed to people and agents ("Do not work around this; ask the
  owner to unlock it, which requires their administrator password") and never a
  command that would disable it. The repository's own flag, rewrite and hook
  become mirrors the app re-applies whenever they are removed, with an event
  log; the helper keeps an administrator-only audit log. Doctor checks every
  layer. What it does **not** stop, stated in the app: a user of the account
  who deliberately sets `GIT_CONFIG_NOSYSTEM=1`, gives the remote an explicit
  `pushurl`, supplies their own `git-remote-gitswitch-push-blocked`, uses another
  git binary, or copies the repository elsewhere — git's own manual says the
  system scope is protected *by* the environment, not *against* the user. The
  lock is purely local; nothing is sent to or configured on GitHub. To remove
  everything by hand as an administrator:
  `sudo /Library/PrivilegedHelperTools/com.gitswitch.lock-helper --uninstall-all`
  (Linux: `sudo /usr/local/lib/gitswitch/lock-helper --uninstall-all`; Windows,
  elevated: `C:\ProgramData\GitSwitch\lock\bin\gitswitch-lock-helper.exe --uninstall-all`).
  Dragging the app to the Trash does not run that.

  | | macOS | Linux | Windows |
  |---|---|---|---|
  | Where the rule lives | `/etc/gitconfig` → `/etc/gitswitch/` (root:wheel) | `/etc/gitconfig` → `/etc/gitswitch/` (root:root) | `<Git for Windows>\etc\gitconfig` → `C:\ProgramData\GitSwitch\lock` (Administrators-only DACL) |
  | The prompt | macOS administrator name + password (`osascript`) | polkit agent password; `sudo -A` or the exact `sudo` command over ssh | UAC: a consent click on an administrator account, a password on a standard one (Doctor says which) |
  | Proven by | `scripts/verify/lock.sh` (test root) on every push, the real helper as root on the macOS runner, and the manual checklist for the dialog | the real helper as root on the Ubuntu runner | the real helper elevated on the Windows runner (UAC is off there, so the dialog itself is a manual check) |
- **History** — browse branches (local/remote, ahead/behind), paginated commit
  history with a real commit graph (lanes, forks and merges drawn the way
  `git log --graph` does), per-commit detail with files changed and signature
  status, message search, a **GitHub account filter** (narrow the repo list to
  one profile, and optionally show only that identity's commits), and a merge
  panel showing which branches already contain the one you're looking at.
  **Fetch** shows where your branch and its remote each point and which
  commits you haven't pulled yet — it never merges or touches your working
  tree, and verifies that before reporting success.
  - **Go to commit.** Paste a commit id, short id or ref to open it. Every
    commit's detail offers: *Go back to this commit* — look at it (detached),
    or move the branch there keeping the later changes unstaged, staged, or
    dropping them (a backup branch first, the undo command afterwards); *Start
    a branch here*; *Undo this commit* (a revert, with the side to keep when it
    is a merge); *Cherry-pick onto* the current branch; *Copy hash*; and the
    line diff of every file it touched. A reset that would drop commits already
    on the upstream is refused and the alternatives are offered instead.
    Anything that ends in a conflict hands you to the Changes page.
  - **Fetch vs Check GitHub.** *Fetch* runs `git fetch` in the folder: it
    updates your copy of the remote branches (`origin/*`) and nothing else —
    your branch, your files and your unpushed commits stay put, nothing is
    merged. *Check GitHub* asks the remote where the branch is without
    downloading anything at all (no objects, no remote refs, not even
    `FETCH_HEAD`), says how many commits are new since your last fetch when
    that can be known (the objects are already here, or a signed-in GitHub
    account can answer), and offers *Fetch now*.
  - **Submodules.** A repository's populated submodules are listed as chips
    with their own ahead/behind; pick one and the page browses that submodule
    (branches, commits, Fetch and every commit action belong to it) until you
    go back to the repository. Fetch on the repository itself updates its
    remote branches only; git fetches submodules on demand.
- **Commit Audit** — finds commits authored with the wrong identity, rewrites the
  ones you haven't pushed (keeping a backup branch, never touching published
  history), and can install a `pre-commit` guard that blocks future mistakes.
- **Commit signing** — per profile, turn on SSH signing (`gpg.format=ssh`) for
  GitHub's *Verified* badge. GitSwitch keeps `~/.ssh/allowed_signers` in sync
  automatically, which is the step that's easy to get wrong by hand.
- **SSH key wizard** — generate Ed25519 keys, register them on a GitHub account
  in one click (via `gh`), test the connection, delete old keys.
- **GitHub sign-in** — device-flow OAuth *and* instant sign-in via your
  existing GitHub CLI (`gh`) logins; autofill profile name/email from an account.
- **Tray icon**, toast feedback, and a Settings view of the managed gitconfig.
  Pages remember where you were (selected repo, branch, filters) between visits.

## VS Code extension

`vscode-extension/` is a companion extension that surfaces the active identity
where you actually commit — a status-bar readout, an amber warning when a repo's
git identity doesn't match its GitSwitch profile, and a fetch-only "what's
incoming" check.

It reads the same `profiles.json` this app writes rather than reimplementing
anything, so the two can't drift apart.

```bash
cd vscode-extension && npm install && npm run package
code --install-extension gitswitch-0.1.0.vsix
```

## Requirements

- **git** (required)
- **ssh / ssh-keygen** (preinstalled on macOS, Linux, Windows 10+)
- **gh** — the [GitHub CLI](https://cli.github.com) (optional; powers the
  one-click key registration, CLI sign-in, and autofill)

## Development

```bash
npm install
npm run tauri dev     # run the full app from source (hot reload)
```

## Building installers

See **[BUILD.md](BUILD.md)** for per-OS builds, the Docker-based Linux build,
and the CI workflow that produces macOS/Windows/Linux installers from a git tag.

```bash
npm run tauri build                    # installer for this machine's OS
docker compose run --rm builder        # Linux .deb/.AppImage/.rpm from any machine
```

## Testing

```bash
cd src-tauri && cargo test             # Rust unit tests
```
