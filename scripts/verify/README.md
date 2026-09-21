# Verification suites

End-to-end checks that exercise the real backend and the built frontend, beyond
`cargo test`. They exist so that "does everything still work?" has a repeatable
answer instead of a manual click-through.

| Script | What it proves | Needs |
|---|---|---|
| `sandbox.sh` | The Changes page operations against **real git** in throwaway repos: staging awkward filenames, discard never touching ignored files, commit hooks, amend refused on pushed commits, all three pull modes incl. conflicts, push blocking from a plain terminal (`git push`, `--no-verify`, a literal URL), hook chaining, submodule update | git, cargo |
| `regression.sh` | Features from before the Changes page: full/sparse clone, clone with submodules, the commit identity guard (install / chain / uninstall), History branches/graph/sync | git, cargo |
| `submodules.sh` | Submodule reporting on the shape that makes `git submodule status` **fatal** (a gitlink with no `.gitmodules` entry), plus moved / dirty / uninitialised submodules | git, cargo |
| `harness/ui.mjs` | The Changes page in every repo state against a mocked backend: pull consequence sentences, commit blockers, push-blocked wording, diff panel, discard confirmation, job survival across page switches, submodule card, no overflow at half-screen widths | `npm run build`, Chrome |
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

## Headless UI harness

```bash
cd scripts/verify/harness && npm install   # puppeteer-core only
node ui.mjs
```

It launches the system Chrome (`/Applications/Google Chrome.app`; override with
`CHROME=/path/to/chrome`). Nothing is installed into the app's own
`package.json`.
