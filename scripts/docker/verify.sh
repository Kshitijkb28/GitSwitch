#!/bin/bash
# The verification suites on Linux, in Docker (Ubuntu 22.04, as on CI), from
# this tree as it is now: uncommitted work included, ignored files left out,
# the tree itself never touched.
#
#   bash scripts/docker/verify.sh                                # every suite (all.sh)
#   bash scripts/docker/verify.sh bash scripts/verify/netspeed.sh
#   VERIFY_EXCLUDE="my-notes other-dir" bash scripts/docker/verify.sh   # untracked folders to leave out
#
# Left out unless VERIFY_EXCLUDE says otherwise: windows-kit (an untracked
# folder on the Windows PC holding a claude-home/ that must not be copied).
#
# The image build log goes to dist-linux/verify-build.log.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
list="$(mktemp)"
trap 'rm -f "$list"' EXIT
git ls-files -co --exclude-standard | while IFS= read -r f; do
  keep=1
  for e in ${VERIFY_EXCLUDE-windows-kit}; do
    case "$f" in "$e" | "$e"/*) keep=0 ;; esac
  done
  if [ "$keep" = 1 ] && [ -e "$f" ]; then printf '%s\n' "$f"; fi
done > "$list"
mkdir -p dist-linux
echo "building the Linux test image ($(wc -l < "$list" | tr -d ' ') files; log: dist-linux/verify-build.log)"
tar -cf - -T "$list" | docker build --progress=plain -f scripts/docker/Dockerfile.verify -t gitswitch-verify - > dist-linux/verify-build.log 2>&1 \
  || { tail -n 40 dist-linux/verify-build.log; exit 1; }
if [ $# -eq 0 ]; then set -- bash scripts/verify/all.sh; fi
# Chrome needs more shared memory than Docker's 64 MB default.
exec docker run --rm --shm-size=2g gitswitch-verify "$@"
