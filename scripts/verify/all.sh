#!/bin/bash
# Run every verification suite and summarise. Exit non-zero if any fails.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
status=0
# node may be installed yet broken (a Homebrew node with missing dylibs is how
# this repo met bun); bun runs the same scripts.
if node --version >/dev/null 2>&1; then JS=node; TSC="npx tsc"; BUILD="npm run build"
elif command -v bun >/dev/null 2>&1; then JS=bun; TSC="bun node_modules/typescript/bin/tsc"; BUILD="bun run build"
else echo "neither node nor bun works; the frontend checks cannot run"; JS=""; TSC="false"; BUILD="false"; fi
run() {
  local name="$1"; shift
  printf '\n\033[1m════ %s ════\033[0m\n' "$name"
  if "$@"; then :; else status=1; fi
}

run "Lock helper sidecar" bash "$ROOT/scripts/build-lock-helper.sh" debug
run "Rust unit tests"     bash -c "cd '$ROOT/src-tauri' && cargo test --workspace 2>&1 | grep -E '^test result' | head -6 && cargo test --workspace >/dev/null 2>&1"
# Both the warning count AND clippy's own exit code: a crate that fails to
# compile has zero warnings, which used to read as a pass.
run "Clippy"              bash -c "cd '$ROOT/src-tauri' && out=\$(cargo clippy --workspace --all-targets 2>&1); rc=\$?; n=\$(printf '%s\n' \"\$out\" | grep -cE '^warning: '); echo \"warnings: \$n (clippy exit \$rc)\"; [ \$rc -eq 0 ] || printf '%s\n' \"\$out\" | tail -30; [ \$rc -eq 0 ] && [ \"\$n\" -eq 0 ]"
run "TypeScript"          bash -c "cd '$ROOT' && $TSC --noEmit && echo clean"
# Deprecated in @types/react 19.2: a form submit is typed SubmitEvent<HTMLFormElement>.
run "No deprecated React event types" bash -c "cd '$ROOT' && if grep -rnwE 'FormEvent|FormEventHandler' src; then echo 'type a form submit as SubmitEvent<HTMLFormElement>'; false; else echo clean; fi"
run "Frontend build"      bash -c "cd '$ROOT' && $BUILD 2>&1 | grep -E '✓ built'"
run "IPC contract"        python3 "$HERE/harness/ipc.py"
run "Packaging and install docs" python3 "$HERE/harness/packaging.py"
# The VS Code extension: its types, and its tests (resolving a repository's
# profile as the app does; a fetch stopped only on silence). bun runs the
# TypeScript tests as they are; node runs what the compile writes.
ext_suite() (
  cd "$ROOT/vscode-extension" || exit 1
  if [ "$JS" = bun ]; then bun node_modules/typescript/bin/tsc -p . --noEmit && bun src/test/resolve.test.ts && bun src/test/gitrun.test.ts
  else npx tsc -p . && node out/test/resolve.test.js && node out/test/gitrun.test.js; fi
)
if [ -d "$ROOT/vscode-extension/node_modules/typescript" ] && [ -n "$JS" ]; then
  run "VS Code extension" ext_suite
else
  printf '
[1m════ VS Code extension ════[0m
'
  echo "SKIPPED: the extension's TypeScript is not installed (cd vscode-extension && bun install). Set GITSWITCH_ALLOW_SKIP=1 to accept that knowingly."
  [ "${GITSWITCH_ALLOW_SKIP:-}" = 1 ] || status=1
fi
run "Changes sandbox"     bash "$HERE/sandbox.sh"
run "Regression"          bash "$HERE/regression.sh"
run "Identity (gitconfig round-trip)" bash "$HERE/identity.sh"
run "Find repositories"   bash "$HERE/findrepos.sh"
run "Folders without a repository" bash "$HERE/folders.sh"
run "Hostile repositories" bash "$HERE/hostile.sh"
run "Uninstalling GitSwitch" bash "$HERE/uninstall.sh"
run "Uninstall tab (apps and leftovers)" bash "$HERE/apps.sh"
run "GitHub pull requests" bash "$HERE/github.sh"
run "Submodules"          bash "$HERE/submodules.sh"
run "Git LFS"             bash "$HERE/lfs.sh"
run "Sync"                bash "$HERE/sync.sh"
run "Tree (branches, undo, reset, revert, conflicts)" bash "$HERE/tree.sh"
run "Stashes"             bash "$HERE/stash.sh"
run "States"              bash "$HERE/states.sh"
run "Push lock"           bash "$HERE/lock.sh"
run "Terminal"            bash "$HERE/console.sh"
run "Ports"               bash "$HERE/ports.sh"
run "Workspaces"          bash "$HERE/workspace.sh"
run "Net speed (start at sign-in)" bash "$HERE/netspeed.sh"
if [ -d "$HERE/harness/node_modules/puppeteer-core" ] && [ -n "$JS" ]; then
  # One run each: the log's tail is the summary, its exit code the verdict
  # (running twice for the two doubled the time and the exposure to a flaky Chrome start).
  harness() { local out; out="$(mktemp)"; (cd "$HERE/harness" && $JS "$1" >"$out" 2>&1); local rc=$?; tail -n "$([ $rc -eq 0 ] && echo 1 || echo 12)" "$out"; rm -f "$out"; return $rc; }
  run "Headless UI"           harness ui.mjs
  run "Headless UI (History)" harness history.mjs
  run "Layout sweep"          harness sweep.mjs
  # The terminal drawer measured in the system's WebKit, the engine the app
  # runs in on macOS; says SKIPPED (and passes) anywhere else.
  run "Terminal in WebKit"    bash "$HERE/webkit.sh"
  # The two pages against the REAL backend (probe's `invoke` op) in a sandbox: needs git, cargo and Chrome.
  run "Live UI ↔ backend"     harness live.mjs
else
  printf '\n\033[1m════ Headless UI ════\033[0m\n'
  echo "SKIPPED: puppeteer-core is not installed (cd scripts/verify/harness && bun install) or no JS runtime works."
  echo "Five suites did not run (the four browser harnesses and the terminal in WebKit), so this is not a full pass. Set GITSWITCH_ALLOW_SKIP=1 to accept that knowingly."
  [ "${GITSWITCH_ALLOW_SKIP:-}" = 1 ] || status=1
fi

printf '\n\033[1m%s\033[0m\n' "$([ $status -eq 0 ] && echo 'ALL SUITES PASSED' || echo 'SOME SUITES FAILED')"
exit $status
