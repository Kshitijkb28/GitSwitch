#!/usr/bin/env python3
"""What someone who downloads GitSwitch gets: checked from the files that make it.

- The VS Code extension packs its compiled code: `vscode:prepublish` compiles
  before vsce packs, `main` is what the compile writes, and .vscodeignore does
  not leave it out. CI packs it, checks the entry point is inside and attaches
  the .vsix to the release.
- The macOS app is sealed (ad hoc) when bundled, and CI checks the bundle.
- The install docs say how to open a build macOS has not notarized, and name
  the installers the bundler actually writes.

Static on purpose: node and npm do not run everywhere this gate does, and a
release build takes many minutes. CI runs the real packaging.
"""
import fnmatch
import json
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
EXT = f"{ROOT}/vscode-extension"
failed = 0


def ok(label, good, detail=""):
    global failed
    if good:
        print(f"  PASS {label}")
    else:
        failed += 1
        print(f"  FAIL {label}" + (f"\n     {detail}" if detail else ""))


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


# --- The VS Code extension ---------------------------------------------------
pkg = json.loads(read(f"{EXT}/package.json"))
scripts = pkg.get("scripts", {})
pre = scripts.get("vscode:prepublish", "")
compiles = "tsc -p" in pre or ("npm run compile" in pre and "tsc -p" in scripts.get("compile", ""))
ok("the extension's package step compiles it first (vscode:prepublish)", compiles, f"vscode:prepublish = {pre!r}")

tsconfig = json.loads(re.sub(r"//[^\n]*", "", read(f"{EXT}/tsconfig.json")))
opts = tsconfig.get("compilerOptions", {})
out_dir = opts.get("outDir", "out").strip("./")
root_dir = opts.get("rootDir", "src").strip("./")
ok(
    "  its entry point is what the compile writes",
    pkg.get("main") == f"./{out_dir}/extension.js" and os.path.isfile(f"{EXT}/{root_dir}/extension.ts"),
    f"main = {pkg.get('main')!r}, outDir = {out_dir!r}",
)
ignore = [l.strip() for l in read(f"{EXT}/.vscodeignore").splitlines() if l.strip() and not l.startswith("#")]
hidden = [p for p in ignore if not p.startswith("!") and fnmatch.fnmatch(f"{out_dir}/extension.js", p)]
ok("  and .vscodeignore does not leave it out", not hidden, f"matched by {hidden}")

wf = read(f"{ROOT}/.github/workflows/build.yml")
ext_job = re.search(r"\n  verify-extension:\n(.*?)(?=\n  [a-z][\w-]*:\n|\Z)", wf, re.S)
job = ext_job.group(1) if ext_job else ""
ok(
    "CI packs the extension, checks its entry point is inside, and keeps the .vsix",
    "working-directory: vscode-extension" in job
    and "npm run package" in job
    and "extension/out/extension.js" in job
    and "*.vsix" in job
    and "upload-artifact" in job,
    "no verify-extension job doing all of that",
)
rel = re.search(r"\n  release-extension:\n(.*?)(?=\n  [a-z][\w-]*:\n|\Z)", wf, re.S)
ok(
    "  and attaches it to the release on a tag",
    bool(rel) and "github.ref_type == 'tag'" in rel.group(1) and "gh release upload" in rel.group(1) and ".vsix" in rel.group(1),
)

# --- The macOS app --------------------------------------------------------------
conf = json.loads(read(f"{ROOT}/src-tauri/tauri.conf.json"))
ok(
    "every macOS bundle is sealed (ad hoc) when it is built",
    (conf.get("bundle", {}).get("macOS") or {}).get("signingIdentity") == "-",
)
ok(
    "  and CI checks the bundle it built",
    "codesign --verify --deep --strict" in wf and "Signature=adhoc" in wf,
)

# --- The install docs ----------------------------------------------------------
build = read(f"{ROOT}/BUILD.md")
m = re.search(r"\n## 1\.(.*?)\n## 2\.", build, re.S)
section1 = m.group(1) if m else ""
ok("BUILD.md tells a Mac user how to open a downloaded build", "Open Anyway" in section1 and "xattr -dr com.apple.quarantine /Applications/GitSwitch.app" in section1)
product = conf.get("productName", "")
names = [f"{product}_x.y.z_amd64.deb", f"{product}_x.y.z_amd64.AppImage", f"{product}-x.y.z-1.x86_64.rpm", f"{product}_x.y.z_x64-setup.exe"]
missing = [n for n in names if n not in section1]
ok("  and names the installers the bundler writes", not missing, f"missing {missing}")
readme = read(f"{ROOT}/README.md")
ok("README points to the first-open steps", "BUILD.md#first-open-on-macos" in readme)
ok("  and to the extension's .vsix on the release", "gitswitch-<version>.vsix" in readme)

print(f"\nRESULT  {'all passed' if not failed else f'{failed} failed'}")
sys.exit(1 if failed else 0)
