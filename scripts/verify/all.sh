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
run "Rust unit tests"     bash -c "cd '$ROOT/src-tauri' && cargo test 2>&1 | grep -E '^test result' | head -2 && cargo test >/dev/null 2>&1"
run "Clippy"              bash -c "cd '$ROOT/src-tauri' && n=\$(cargo clippy --all-targets 2>&1 | grep -cE '^warning: '); echo \"warnings: \$n\"; [ \"\$n\" -eq 0 ]"
run "TypeScript"          bash -c "cd '$ROOT' && $TSC --noEmit && echo clean"
run "Frontend build"      bash -c "cd '$ROOT' && $BUILD 2>&1 | grep -E '✓ built'"
run "IPC contract"        python3 "$HERE/harness/ipc.py"
run "Changes sandbox"     bash "$HERE/sandbox.sh"
run "Regression"          bash "$HERE/regression.sh"
run "Submodules"          bash "$HERE/submodules.sh"
run "Git LFS"             bash "$HERE/lfs.sh"
run "Sync"                bash "$HERE/sync.sh"
run "Tree (branches, undo, reset, revert, conflicts)" bash "$HERE/tree.sh"
run "Stashes"             bash "$HERE/stash.sh"
run "Push lock"           bash "$HERE/lock.sh"
if [ -d "$HERE/harness/node_modules/puppeteer-core" ] && [ -n "$JS" ]; then
  # One run each: the log's tail is the summary, its exit code the verdict
  # (running twice for the two doubled the time and the exposure to a flaky Chrome start).
  harness() { local out; out="$(mktemp)"; (cd "$HERE/harness" && $JS "$1" >"$out" 2>&1); local rc=$?; tail -n "$([ $rc -eq 0 ] && echo 1 || echo 12)" "$out"; rm -f "$out"; return $rc; }
  run "Headless UI"           harness ui.mjs
  run "Headless UI (History)" harness history.mjs
  run "Layout sweep"          harness sweep.mjs
else
  echo; echo "(headless UI skipped — run: cd scripts/verify/harness && npm install   (or: bun install))"
fi

printf '\n\033[1m%s\033[0m\n' "$([ $status -eq 0 ] && echo 'ALL SUITES PASSED' || echo 'SOME SUITES FAILED')"
exit $status
