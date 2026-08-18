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
- **Sparse Clone** — metadata-only clone of a huge repo, then tick just the
  folders you need (`git sparse-checkout`, with add-later support).
- **Push blocking** — mark a profile read-only; `git push` fails locally in its
  folders while pull/fetch keep working.
- **History** — browse branches (local/remote, ahead/behind), paginated commit
  history with a real commit graph (lanes, forks and merges drawn the way
  `git log --graph` does), per-commit detail with files changed and signature
  status, message search, and a merge panel showing which branches already
  contain the one you're looking at.
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
