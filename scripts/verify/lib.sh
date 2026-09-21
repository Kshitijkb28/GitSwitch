# Shared plumbing for the git verification suites. Source it; don't run it.
#
# Every suite must call `verify_init <suite-name>` FIRST. It redirects HOME to a
# throwaway directory before anything else happens — moving that below a config
# write once overwrote the real ~/.gitconfig, which is why it lives here and
# not in each script.

verify_init() {
  local suite="$1"
  VERIFY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  ROOT="$(cd "$VERIFY_DIR/../.." && pwd)"
  SRC="$ROOT/src-tauri"
  WORK="${GITSWITCH_VERIFY_TMP:-${TMPDIR:-/tmp}/gitswitch-verify}"
  SB="$WORK/$suite"
  REAL_HOME="$HOME"                 # cargo needs the real one; git must never see it
  rm -rf "$SB"; mkdir -p "$SB/home"
  export HOME="$SB/home"            # FIRST — before any line that writes a gitconfig
  export GIT_CONFIG_NOSYSTEM=1
  cat > "$HOME/.gitconfig" <<EOF
[user]
	name = ${2:-Verify User}
	email = ${3:-verify@example.test}
[init]
	defaultBranch = main
# The "remotes" here are local paths. git blocks the file transport for
# submodules by default (CVE-2022-39253); allow it inside the sandbox only.
[protocol "file"]
	allow = always
EOF

  PASS=0; FAIL=0
  BIN="$(cd "$SRC" && HOME="$REAL_HOME" cargo test --lib --no-run --message-format=json 2>/dev/null | python3 -c "
import sys,json
for l in sys.stdin:
    try: d=json.loads(l)
    except: continue
    if d.get('profile',{}).get('test') and d.get('executable'): print(d['executable'])
" | tail -1)"
  [ -n "$BIN" ] || { echo "could not build the test binary (is src-tauri/src/probe.rs registered in lib.rs?)"; exit 1; }
}

# probe <op> <repo> [comma-separated args] -> the JSON the backend returned
probe() {
  PROBE_OP="$1" PROBE_REPO="$2" PROBE_ARGS="${3:-}" "$BIN" probe --ignored --nocapture 2>/dev/null \
    | sed -n 's/^PROBE_OUT //p'
}

# jqf <dotted.path>  — read one field from JSON on stdin (python, no jq needed)
jqf() {
  python3 -c "
import sys,json
d=json.load(sys.stdin)
for k in '$1'.split('.'):
    if d is None: break
    d=d.get(k) if isinstance(d,dict) else None
print(json.dumps(d) if isinstance(d,(dict,list)) else d)
"
}

# ok <label> <actual> <expected-substring>
ok() {
  if printf '%s' "$2" | grep -qF -- "$3"; then
    PASS=$((PASS+1)); printf '  \033[32mPASS\033[0m %s\n' "$1"
  else
    FAIL=$((FAIL+1)); printf '  \033[31mFAIL\033[0m %s\n     got: %s\n     want: %s\n' "$1" "$(printf '%s' "$2" | head -c 400)" "$3"
  fi
}

section() { printf '\n\033[1m== %s\033[0m\n' "$1"; }

verify_result() {
  printf '\n\033[1mRESULT\033[0m  %d passed, %d failed\n' "$PASS" "$FAIL"
  [ "$FAIL" -eq 0 ]
}
