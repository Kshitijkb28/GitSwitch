#!/bin/bash
# Run every verification suite and summarise. Exit non-zero if any fails.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
status=0
run() {
  local name="$1"; shift
  printf '\n\033[1m════ %s ════\033[0m\n' "$name"
  if "$@"; then :; else status=1; fi
}

run "Rust unit tests"     bash -c "cd '$ROOT/src-tauri' && cargo test 2>&1 | grep -E '^test result' | head -1 && cargo test >/dev/null 2>&1"
run "Clippy"              bash -c "cd '$ROOT/src-tauri' && n=\$(cargo clippy --all-targets 2>&1 | grep -cE '^warning: '); echo \"warnings: \$n\"; [ \"\$n\" -eq 0 ]"
run "TypeScript"          bash -c "cd '$ROOT' && npx tsc --noEmit && echo clean"
run "Frontend build"      bash -c "cd '$ROOT' && npm run build 2>&1 | grep -E '✓ built'"
run "IPC contract"        python3 "$HERE/harness/ipc.py"
run "Changes sandbox"     bash "$HERE/sandbox.sh"
run "Regression"          bash "$HERE/regression.sh"
run "Submodules"          bash "$HERE/submodules.sh"
if [ -d "$HERE/harness/node_modules/puppeteer-core" ]; then
  run "Headless UI"       bash -c "cd '$HERE/harness' && node ui.mjs 2>&1 | tail -1 && node ui.mjs >/dev/null 2>&1"
  run "Layout sweep"      bash -c "cd '$HERE/harness' && node sweep.mjs 2>&1 | tail -1 && node sweep.mjs >/dev/null 2>&1"
else
  echo; echo "(headless UI skipped — run: cd scripts/verify/harness && npm install)"
fi

printf '\n\033[1m%s\033[0m\n' "$([ $status -eq 0 ] && echo 'ALL SUITES PASSED' || echo 'SOME SUITES FAILED')"
exit $status
