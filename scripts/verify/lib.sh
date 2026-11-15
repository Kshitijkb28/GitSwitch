# Shared plumbing for the git verification suites. Source it; don't run it.
#
# Every suite must call `verify_init <suite-name>` FIRST. It redirects HOME to a
# throwaway directory before anything else happens — moving that below a config
# write once overwrote the real ~/.gitconfig, which is why it lives here and
# not in each script.

# Portable helpers: the suites also run on GitHub's Linux and Windows runners
# (Git Bash), where python3/shasum may be missing and native binaries cannot
# open MSYS paths (/c/Users/…) handed to them through environment variables.
PY="$(command -v python3 || command -v python)"
# Python reads and writes UTF-8, and ends lines with "\n", on every runner. On
# Windows its pipes default to the ANSI code page (cp1252: the backend's UTF-8
# "café" was read as "cafÃ©") and print() writes "\r\n", whose CR survives
# $(...); pyenv/sitecustomize.py sets the line ends there. And no bytecode:
# importing it would leave a __pycache__ folder inside the repository.
export PYTHONUTF8=1 PYTHONDONTWRITEBYTECODE=1
if command -v cygpath >/dev/null 2>&1; then
  PYTHONPATH="$(cygpath -w "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pyenv")${PYTHONPATH:+;$PYTHONPATH}"
  export PYTHONPATH
fi
sha256file() { if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1; else sha256sum "$1" | cut -d' ' -f1; fi; }
# realdir <dir> -> the real, native path of a directory (symlinks resolved;
# Windows: long-name mixed form C:/Users/…)
realdir() { if command -v cygpath >/dev/null 2>&1; then cygpath -m -l "$(cd "$1" && pwd -P)"; else (cd "$1" && pwd -P); fi; }

verify_init() {
  local suite="$1"
  VERIFY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  ROOT="$(cd "$VERIFY_DIR/../.." && pwd)"
  SRC="$ROOT/src-tauri"
  local tmp="${TMPDIR:-${TEMP:-/tmp}}"; tmp="${tmp%/}"     # macOS TMPDIR ends in a slash
  WORK="${GITSWITCH_VERIFY_TMP:-$tmp/gitswitch-verify}"
  SB="$WORK/$suite"
  REAL_HOME="$HOME"                 # cargo needs the real one; git must never see it
  rm -rf "$SB"; mkdir -p "$SB/home"
  if command -v cygpath >/dev/null 2>&1; then SB="$(cygpath -m -l "$SB")"; fi
  export HOME="$SB/home"            # FIRST — before any line that writes a gitconfig
  export GIT_CONFIG_NOSYSTEM=1
  # Tokens (a kept "Sign in with GitHub") go to files here, never the real
  # keychain: the debug build honours this, a release build ignores it.
  export GITSWITCH_TOKEN_DIR="$SB/tokens"
  # The GitHub CLI's sign-ins stay out of reach too: on Windows gh keeps them
  # under %APPDATA%, not HOME, and a suite once had the real gh hand this
  # machine's token to its fake API. A token in the environment would win over
  # any config, so none is passed on (a check that wants one sets it itself).
  export GH_CONFIG_DIR="$HOME/.config/gh"
  unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
  # The XDG config folder inside the sandbox too: git reads
  # $XDG_CONFIG_HOME/git/config, and Linux keeps start-up entries under it.
  export XDG_CONFIG_HOME="$HOME/.config"
  # Starting at sign-in (the Net speed tab): a suite's entry goes to a test
  # key (Windows) or a folder in the sandbox, never the real one. The debug
  # build honours these; a test build without them has nowhere to write.
  if command -v cygpath >/dev/null 2>&1; then
    export GITSWITCH_AUTOSTART_KEY="Software\\GitSwitchVerify\\$suite"
  elif [ "$(uname -s)" = Darwin ]; then
    export GITSWITCH_AUTOSTART_DIR="$HOME/Library/LaunchAgents"
  else
    export GITSWITCH_AUTOSTART_DIR="$HOME/.config/autostart"
  fi
  # Windows: the app finds its data folder and ~/.ssh through the system, not
  # HOME, so without these a suite's profiles land in the real store (they
  # once showed up as "Ports" and "Workspaces" in the installed app).
  if command -v cygpath >/dev/null 2>&1; then
    export GITSWITCH_DATA_DIR="$HOME/AppData/Roaming" GITSWITCH_HOME="$HOME"
  fi
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
  # tauri-build refuses to compile the app unless the lock-helper sidecar file
  # exists, so build it (debug) before the app's test binary.
  local build_log; build_log="$(mktemp)"
  (HOME="$REAL_HOME" bash "$ROOT/scripts/build-lock-helper.sh" debug >"$build_log" 2>&1) || { echo "could not build the lock helper sidecar:"; tail -n 30 "$build_log"; exit 1; }
  HELPER_BUNDLED="$SRC/target/debug/gitswitch-lock-helper"; [ -x "$HELPER_BUNDLED.exe" ] && HELPER_BUNDLED="$HELPER_BUNDLED.exe"
  BIN="$(cd "$SRC" && HOME="$REAL_HOME" cargo test -p gitswitch --lib --no-run --message-format=json 2>"$build_log" | "$PY" -c "
import sys,json
for l in sys.stdin:
    try: d=json.loads(l)
    except: continue
    if d.get('profile',{}).get('test') and d.get('executable') and d.get('target',{}).get('name')=='gitswitch_lib': print(d['executable'])
" | tail -1)"
  [ -n "$BIN" ] || { echo "could not build the test binary (is src-tauri/src/probe.rs registered in lib.rs?):"; tail -n 30 "$build_log"; exit 1; }
}

# pinvoke <tauri command> <json args as the UI sends them> -> what the command returned
pinvoke() { PROBE_OP=invoke PROBE_CMD="$1" PROBE_JSON="$2" PROBE_TRACE_FILE="$SB/probe-trace.json" "$BIN" probe --ignored --nocapture 2>/dev/null | sed -n 's/^PROBE_OUT //p'; }

# probe <op> <repo> [comma-separated args] -> the JSON the backend returned
# The probe also writes every clone-progress entry the op set to a file next to
# the sandbox (see probe_trace): a clone's phases last milliseconds against a
# local remote, so polling for them from outside would prove nothing.
probe() {
  PROBE_OP="$1" PROBE_REPO="$2" PROBE_ARGS="${3:-}" PROBE_TRACE_FILE="$SB/probe-trace.json" "$BIN" probe --ignored --nocapture 2>/dev/null \
    | sed -n 's/^PROBE_OUT //p'
}

# probe_trace -> the clone-progress entries the LAST probe or pinvoke call set,
# in order, as a JSON list ("[]" when it set none)
probe_trace() { cat "$SB/probe-trace.json" 2>/dev/null || echo '[]'; }

# jqf <dotted.path>  — read one field from JSON on stdin (python, no jq needed)
jqf() {
  "$PY" -c "
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
