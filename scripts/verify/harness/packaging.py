#!/usr/bin/env python3
"""What someone who downloads GitSwitch gets: checked from the files that make it.

- The VS Code extension packs its compiled code: `vscode:prepublish` compiles
  before vsce packs, `main` is what the compile writes, and .vscodeignore does
  not leave it out. CI packs it, checks the entry point is inside and attaches
  the .vsix to the release.
- The macOS app is sealed (ad hoc) when bundled, and CI checks the bundle.
- The install docs say how to open a build macOS has not notarized, and name
  the installers the bundler actually writes.
- In-app updates: a tag must be the app's version and have the signing key;
  every OS attaches its signed installers, and latest.json is written with an
  entry per installer once all are in.

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
# --- Docker runs the app -----------------------------------------------------------
docker = read(f"{ROOT}/Dockerfile")
stages = re.findall(r"^FROM \S+ AS (\S+)", docker, re.M)
ok("the Dockerfile builds the installers by default and runs the app and tauri dev", stages[-1:] == ["builder"] and {"app", "dev"} <= set(stages), f"stages: {stages}")
compose = read(f"{ROOT}/docker-compose.yml")
ok("  compose names each target, and serves the screen on localhost only", all(f"target: {t}" in compose for t in ("builder", "app", "dev")) and compose.count('"127.0.0.1:6080:6080"') == 2 and "0.0.0.0" not in compose)
scripts = [f"{ROOT}/scripts/docker/{n}" for n in ("display.sh", "app.sh", "dev.sh")]
ok("  its scripts exist, are executable and have LF endings", all(os.path.exists(s) and os.access(s, os.X_OK) and b"\r" not in open(s, "rb").read() for s in scripts))
ignore = read(f"{ROOT}/.dockerignore").split("\n")
ok("  the build context leaves out local scratch and host binaries", all(x in ignore for x in (".claude", "src-tauri/binaries", "**/node_modules")), str(ignore))
display = read(f"{ROOT}/scripts/docker/display.sh")
clear_at, xvfb_at = display.find("rm -f /tmp/.X99-lock /tmp/.X11-unix/X99"), display.find("Xvfb :99")
ok("  a restarted container clears the killed display's lock and socket, then waits for its own server", 0 <= clear_at < xvfb_at and "-S /tmp/.X11-unix/X99" in display and 'kill -0 "$xvfb"' in display)
dev_sh = read(f"{ROOT}/scripts/docker/dev.sh")
ok("  tauri dev installs node_modules again when the lockfile changed", "sha256sum package-lock.json" in dev_sh and "node_modules/.lock-sha256" in dev_sh)
gitignore = read(f"{ROOT}/.gitignore").split("\n")
ok("  dev builds its Linux sidecars into a volume, over a folder the checkout keeps (never one root creates)", "gitswitch-sidecars:/app/src-tauri/binaries" in compose and os.path.exists(f"{ROOT}/src-tauri/binaries/.gitkeep") and "!src-tauri/binaries/.gitkeep" in gitignore and "src-tauri/binaries" not in gitignore)

# --- The sidecars build without a shell ------------------------------------------
before = [conf.get("build", {}).get("beforeDevCommand", ""), conf.get("build", {}).get("beforeBuildCommand", "")]
ok("Tauri's before-commands build the sidecars with cargo, not bash (cmd and PowerShell have none)", all("-p gitswitch-sidecars" in c and "bash" not in c for c in before), str(before))
ws = read(f"{ROOT}/src-tauri/Cargo.toml")
ok("  the sidecars crate is a workspace member, outside the default build", '"sidecars"' in ws.split("default-members")[0] and '"sidecars"' not in ws.split("default-members")[1].split("\n")[0], "")

# --- gitswitch:// opens the app -------------------------------------------------
schemes = conf.get("plugins", {}).get("deep-link", {}).get("desktop", {}).get("schemes", [])
ok("the app registers the gitswitch:// scheme in every bundle", "gitswitch" in schemes, f"schemes: {schemes}")
cargo = read(f"{ROOT}/src-tauri/Cargo.toml")
ok("  with the deep-link plugin, and a second launch hands its link to the running app", "tauri-plugin-deep-link" in cargo and 'tauri-plugin-single-instance = { version = "2", features = ["deep-link"] }' in cargo)
ext_src = read(f"{EXT}/src/extension.ts")
deep = read(f"{ROOT}/src-tauri/src/deep_link.rs")
pages = re.findall(r'gitswitch://([a-z]+)', ext_src)
ok("  the extension opens only pages the app's link allowlist names", bool(pages) and all(f'"{p}"' in deep for p in pages), f"extension opens {pages}")

# --- In-app updates ---------------------------------------------------------------
upd = conf.get("plugins", {}).get("updater", {})
ok(
    "installed copies look for updates on the latest published release",
    upd.get("pubkey") and upd.get("endpoints") == ["https://github.com/Kshitijkb28/GitSwitch/releases/latest/download/latest.json"],
    f"updater: {upd}",
)
ok("  local builds need no signing key (the updater files are CI's, through an overlay)", not conf.get("bundle", {}).get("createUpdaterArtifacts"))
overlay = json.loads(read(f"{ROOT}/src-tauri/tauri.updater.conf.json"))
build_job = re.search(r"\n  build:\n(.*?)(?=\n  [a-z][\w-]*:\n|\Z)", wf, re.S)
bj = build_job.group(1) if build_job else ""
ok(
    "  a tagged build signs them with the release key, and a tag without the key fails",
    overlay.get("bundle", {}).get("createUpdaterArtifacts") is True
    and "--config src-tauri/tauri.updater.conf.json" in bj
    and "TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}" in bj
    and 'elif [ "$GITHUB_REF_TYPE" = tag ]' in bj,
)
ok("  a tag must be the version every file carries", 'node scripts/release/version.mjs --check "$GITHUB_REF_NAME"' in bj)
ok(
    "  each platform attaches its installers' signatures, and latest.json is written once all are in",
    "includeUpdaterJson: false" in bj
    and "release/bundle/**/*.sig" in bj
    and bool(re.search(r"\n  release-updater:\n(?:.*\n)*?    needs: \[build\]\n(?:.*\n)*?.*scripts/release/updater-manifest\.mjs(?:.*\n)*?.*--require windows-x86_64-nsis,windows-x86_64-msi,darwin-aarch64-app,darwin-x86_64-app,linux-x86_64-appimage,linux-x86_64-deb,linux-x86_64-rpm", wf)),
)
ok("  for every installer the bundle makes on each OS", conf.get("bundle", {}).get("targets") == "all")

readme = read(f"{ROOT}/README.md")
ok("README points to the first-open steps", "BUILD.md#first-open-on-macos" in readme)
ok("  and to the extension's .vsix on the release", "gitswitch-<version>.vsix" in readme)

print(f"\nRESULT  {'all passed' if not failed else f'{failed} failed'}")
sys.exit(1 if failed else 0)
