# Verification suites

End-to-end checks that exercise the real backend and the built frontend, beyond
`cargo test`. They exist so that "does everything still work?" has a repeatable
answer instead of a manual click-through.

| Script | What it proves | Needs |
|---|---|---|
| `sandbox.sh` | The Changes page operations against **real git** in throwaway repos: staging awkward filenames, discard never touching ignored files, commit hooks, amend refused on pushed commits, all three pull modes incl. conflicts, push blocking from a plain terminal (`git push`, `--no-verify`, a literal URL), hook chaining, submodule update | git, cargo |
| `regression.sh` | Features from before the Changes page: full/sparse clone, clone with submodules, the commit identity guard (install / chain / uninstall), History branches/graph/sync | git, cargo |
| `submodules.sh` | Submodule reporting on the shape that makes `git submodule status` **fatal** (a gitlink with no `.gitmodules` entry), plus moved / dirty / uninitialised submodules | git, cargo |
| `lfs.sh` | Git LFS: pointer-stub detection, `git lfs pull` proven against a local bare remote (git-lfs 3.x stores objects in `<bare>/lfs/objects`), the "filters never configured" trap where `git lfs pull` exits 0 and does nothing, git-lfs missing from the machine (including the `git status` failure that reads like a network outage), objects missing from the server, and the Clone page's "Also download Git LFS files" option for full and sparse clones | git, git-lfs, cargo |
| `sync.sh` | "The latest from upstream, your commits on top" against real git: a superproject with submodules A and B, an unmapped gitlink, an upstream that moves both submodules and the superproject, a local clone with its own commit in A sealed by a superproject commit, uncommitted edits and untracked files in both. Assess names every action and its base; the run leaves own commits on top in the superproject **and** in A, fast-forwards B, restores dirty paths, makes backup branches. Variants: a file conflict (pause → abort restores A too; resolve → Continue sync), a conflict inside A, a commit upstream already had (dropped and named), unrecorded checkout / two own commits moving one gitlink / detached superproject (blocked), `rebase.updateRefs` and `submodule.recurse` set (pinned away), a detached submodule re-attached, uninitialised and unfetchable submodules, an upstream pointer nobody can reach, a repo without submodules, "Record submodule pointers" | git, cargo |
| `lock.sh` | The push lock, unprivileged: the helper is re-rooted into the sandbox (`GITSWITCH_LOCK_ROOT`, debug builds only) and git's system scope pointed at it (`GIT_CONFIG_SYSTEM`), so bootstrap, lock, unlock, uninstall, every refusal (tampered job, symlinked registry, unmanaged path, lock+unlock in one job), foreign system-gitconfig bytes preserved, then the app driving it with `GITSWITCH_ELEVATE=direct` standing in for the prompt: `push-locked` refusal, explicit-pushurl hook path, mirror self-heal and its event, cancelled/denied/failed/manual outcomes, Doctor findings and fixes. It also asserts that the documented bypasses (`GIT_CONFIG_NOSYSTEM=1`, `-c remote.origin.pushurl=`) really work, so the disclosure stays true | git, cargo |
| `harness/ui.mjs` | The Changes page in every repo state against a mocked backend: pull consequence sentences, commit blockers, push-blocked wording, diff panel, discard confirmation, job survival across page switches, submodule card, the Sync card (plan table, stash toggle re-planning without a fetch, blockers, result table with undo commands, paused/continue/abort, the submodule's view of a paused parent sync), the Clone page's LFS option, no overflow at half-screen widths | `npm run build`, Chrome |
| `harness/sweep.mjs` | Every page at 700 px and 900 px, measuring the element that actually scrolls | `npm run build`, Chrome |
| `harness/ipc.py` | Every `invoke()` in `api.ts` matches a registered `#[tauri::command]` with identical argument names — the bug class that compiles and still fails on click | python3 |

Run everything:

```bash
scripts/verify/all.sh
```

## Safety

- Every git suite redirects `HOME` to a throwaway directory **before** any line
  that writes a config file. Keep it that way: moving that line below a write
  once overwrote the real `~/.gitconfig`.
- Throwaway repos live under `${TMPDIR}/gitswitch-verify`, never inside this
  repository.
- Nothing here uses the network, a GitHub account, or a real SSH key. Local
  bare repos stand in for remotes; `protocol.file.allow=always` is set in the
  sandbox gitconfig only, because git otherwise refuses `file://` submodules
  (CVE-2022-39253) and the suite would be testing git's mitigation rather than
  GitSwitch.
- The git suites drive the backend through `src-tauri/src/probe.rs`, a
  `#[cfg(test)]`-only module exposing the operations to a test binary. It is
  never compiled into the app.
- `lock.sh` never touches the real `/etc`, `/Library` or `/usr/local`: the
  helper honours `GITSWITCH_LOCK_ROOT` only in debug builds and refuses it when
  actually running as root, and the suite points `GIT_CONFIG_SYSTEM` at the
  re-rooted file. The real administrator prompt is covered by the manual
  checklist in `manual-lock-macos.md`.

## Headless UI harness

```bash
cd scripts/verify/harness && npm install   # puppeteer-core only (or: bun install)
node ui.mjs                                # or: bun ui.mjs
```

It launches the system Chrome (`/Applications/Google Chrome.app`; override with
`CHROME=/path/to/chrome`). Nothing is installed into the app's own
`package.json`. `browser.mjs` holds the shared plumbing (serving `dist/`,
launching Chrome, the pass/fail tally); `fixtures.mjs` holds every repo state
the page is checked against, shaped like real repositories seen during
development.
