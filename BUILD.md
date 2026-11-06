# Building & Installing GitSwitch

GitSwitch is a **native desktop app** (Tauri v2 = Rust backend + web UI). One
codebase builds installers for **Windows, macOS, and Linux**.

## Common commands (cheat sheet)

```bash
# Run the full app from source (dev, hot-reload) ...... npm run tauri dev   [native]
# Build a production app for THIS machine's OS ........ npm run tauri build [native]
# Build Linux installers on any machine ............... docker compose run --rm builder
# Run the app in Docker (any machine), in a browser ... docker compose up app   -> localhost:6080
# Run `tauri dev` in Docker on this checkout .......... docker compose up dev   -> localhost:6080
# Build ALL three OSes' installers .................... git tag vX.Y.Z && git push --tags  (CI)
#   (a release the apps update to: version.mjs X.Y.Z first, see "Auto-updater releases")
```

> To **run** GitSwitch from the codebase, use `npm run tauri dev` — or, with
> nothing but Docker installed, `docker compose up dev`.

> **Docker note:** a container has no display of its own, so Docker runs
> GitSwitch on a virtual one and shows it in your browser: `docker compose up
> app` (the built app — the image needs no checkout, so it runs on any Docker
> host) or `docker compose up dev` (`tauri dev` on this checkout), then open
> http://localhost:6080/vnc.html. The screen is served on localhost only. The
> app in a container uses the container's git, keys and folders — mount the
> folders you want it to manage. See [Linux via Docker](#linux-via-docker).

---

## 1. Install a released build (end users)

Download the installer for your OS from the project's GitHub **Releases** page
and run it:

| OS | File | How to install |
|----|------|----------------|
| **macOS** | `GitSwitch_x.y.z_universal.dmg` | Open the `.dmg`, drag GitSwitch to Applications, then see [First open on macOS](#first-open-on-macos) |
| **Windows** | `GitSwitch_x.y.z_x64-setup.exe` or `GitSwitch_x.y.z_x64_en-US.msi` | Run it, click through the installer — SmartScreen asks once, see [Installing](#installing) |
| **Linux (Debian/Ubuntu)** | `GitSwitch_x.y.z_amd64.deb` | `sudo apt install ./GitSwitch_x.y.z_amd64.deb` (pulls in the WebKit/GTK libraries; `dpkg -i` does not) |
| **Linux (any distro)** | `GitSwitch_x.y.z_amd64.AppImage` | `chmod +x GitSwitch_*.AppImage && ./GitSwitch_*.AppImage` |
| **Linux (Fedora/RHEL)** | `GitSwitch-x.y.z-1.x86_64.rpm` | `sudo dnf install ./GitSwitch-x.y.z-1.x86_64.rpm` |

### First open on macOS

The build is sealed but not notarized (that needs a paid Apple Developer ID),
so the first time it is opened macOS asks for confirmation:

- **macOS 15 and later:** open GitSwitch once and click **Done**, then
  **System Settings → Privacy & Security**, find "GitSwitch was blocked…",
  click **Open Anyway**, then **Open**.
- **macOS 14 and earlier:** Control-click GitSwitch in Applications → **Open**
  → **Open**.
- If macOS says the app **is damaged** (a build from before the app was
  sealed): `xattr -dr com.apple.quarantine /Applications/GitSwitch.app`, then
  open it again.

After that it opens like any other app.

### Updating

GitSwitch checks for a newer release shortly after it starts and every six
hours after; when one is out, the sidebar says **Update available**. Install
it from **Settings → About** (or check by hand there). Each copy updates from
the same kind of installer it came from — the Windows setup or `.msi`, the
macOS app, the `.deb`, `.rpm` or AppImage — and only from a release signed
with the project's key. Windows (installed for all users), and the `.deb` and
`.rpm`, ask for administrator rights while installing; GitSwitch restarts
when it is done.

### Runtime requirements (must exist on the user's machine)

GitSwitch drives the system's own git tooling, so these need to be installed:

- **git** — all platforms
- **ssh** + **ssh-keygen** — preinstalled on macOS, Linux, and Windows 10/11
  (OpenSSH). On older Windows, install "OpenSSH Client" from Optional Features.
- **gh** (GitHub CLI) — *optional*, only for the one-click "Register key on
  account" / "Use GitHub CLI" features. https://cli.github.com

---

## 2. Build it yourself (developers)

### Prerequisites (all platforms)
- [Rust](https://rustup.rs) (stable)
- [Node.js](https://nodejs.org) 20+
- Platform build tools (below)

### macOS
```bash
npm install
npm run tauri build         # -> src-tauri/target/release/bundle/{dmg,macos}/
```
Universal (Intel + Apple Silicon):
```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri build -- --target universal-apple-darwin
```

### Windows
```powershell
npm install
npm run tauri build         # -> src-tauri\target\release\bundle\{msi,nsis}\
```
(Needs the "Desktop development with C++" workload from Visual Studio Build Tools.
No bash is needed: the sidecars are built by `cargo run -p gitswitch-sidecars`,
which `npm run tauri dev` and `npm run tauri build` run themselves, from
PowerShell or cmd alike.)

### Linux (native)
```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf
npm install
npm run tauri build         # -> src-tauri/target/release/bundle/{deb,appimage,rpm}/
```

### Linux via Docker
Build the Linux installers on **any** machine (even macOS/Windows) without
installing the Linux toolchain locally:
```bash
docker compose run --rm builder
# installers appear in ./dist-linux/
```
This builds an image and runs a one-shot container that exits immediately. It
does **not** create long-running services or touch any other containers.

> **Architecture note:** the Docker build produces installers for the *host's*
> CPU architecture. On Apple Silicon / ARM machines you get **arm64** Linux
> packages; on Intel/AMD machines you get **x86_64**. To cross-build the other
> arch under emulation (slower; AppImage must be skipped because linuxdeploy
> can't run emulated):
>
> ```bash
> docker build --platform linux/amd64 --build-arg BUNDLES=deb,rpm \
>   -t gitswitch-builder:amd64 .
> docker run --rm --platform linux/amd64 -v "$PWD/dist-linux:/out" gitswitch-builder:amd64
> ```
>
> For clean native builds of both arches (including AppImage), prefer the
> GitHub Actions CI below.

---

## 3. Release all three OSes at once (CI)

`.github/workflows/build.yml` builds macOS + Windows + Linux installers in
parallel. Two ways to trigger it:

- **Tag a release:** `git tag v0.1.0 && git push --tags` → a draft GitHub
  Release is created with every installer attached.
- **Manual:** Actions tab → "Build installers" → Run workflow → download the
  per-OS artifacts.

This is the recommended path: macOS installers can only be built on macOS,
and CI builds all three at once. (The Windows installer can also be
cross-built with Docker — see [Testing on Windows](#6-testing-on-windows).)

## 4. macOS: build + install in one step

```bash
./scripts/install-mac.sh                # build, install to /Applications, clean up
./scripts/install-mac.sh --skip-build   # reinstall the last build
```

This also removes the build-folder `.app`, ejects stray DMG mounts, and fixes
Launch Services registrations so you never end up with "two GitSwitch apps."

## 5. Auto-updater releases

Installed copies read `latest.json` from the **latest published** release
(`plugins.updater.endpoints` in `tauri.conf.json`) and install only files
signed with the key whose public half is `plugins.updater.pubkey` there. The
matching **private key** is at `~/.tauri/gitswitch_updater.key` on the
maintainer's Mac — never commit it, and never replace it: every copy already
installed trusts that one key (and the push-lock helper's signed upgrades use
it too), so a new key would strand them all.

Once, in the repository's **Settings → Secrets and variables → Actions**:
`TAURI_SIGNING_PRIVATE_KEY` (the key file's contents) and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (its password; empty if none).

Each release:

```bash
node scripts/release/version.mjs 0.2.0   # tauri.conf.json, package.json, package-lock.json, Cargo.toml, Cargo.lock
git commit -am "chore: 0.2.0" && git tag v0.2.0 && git push && git push --tags
```

1. CI refuses a tag that is not the version in those files (an update the
   apps would never see, or would be offered again after installing), and a
   tag without the signing key.
2. Each OS builds its installers with the updater's signatures
   (`--config src-tauri/tauri.updater.conf.json`; local builds skip them and
   need no key) and attaches them to the draft release.
3. `release-updater` then writes `latest.json` — an entry per installer
   (`windows-x86_64-nsis`, `-msi`, `darwin-aarch64-app`, `darwin-x86_64-app`,
   `linux-x86_64-appimage`, `-deb`, `-rpm`, plus a plain `<os>-<arch>` each),
   built by `scripts/release/updater-manifest.mjs` — and fails if any platform
   is missing.
4. **Publish** the draft. Until then installed copies see nothing new.

## 6. Testing on Windows

The `.exe` / `.msi` come from the CI workflow. Grab them from the draft
Release, or from the run's artifacts if it was triggered manually.

The `.exe` can also be cross-built from a Linux or macOS machine that has
Docker (no Windows needed): the image brings the Windows SDK through
cargo-xwin and packages the installer with NSIS.

```bash
docker build -f scripts/docker/Dockerfile.windows -t gitswitch-win .
docker run --rm -v "$PWD/dist-windows:/out" gitswitch-win
# -> dist-windows/GitSwitch_x.y.z_x64-setup.exe
```

The result is unsigned either way (SmartScreen asks once, see Installing).

### Installing

1. Run `GitSwitch_x.y.z_x64-setup.exe` (or the `.msi`).
2. SmartScreen will say **"Windows protected your PC"** — the build is
   unsigned, like the unnotarized macOS build. Click **More info → Run anyway**.

### Runtime requirements

| Tool | Needed for | Notes |
|------|------------|-------|
| `git` | everything | [git-scm.com](https://git-scm.com/download/win) — includes the `sh` that runs the commit guard hook |
| `ssh`, `ssh-keygen` | keys, per-folder auth | ships with Windows 10/11 (OpenSSH Client) |
| `gh` | account list, key registration, Auto Assign, autofill | optional — [cli.github.com](https://cli.github.com) |

### What to check first

Windows differs from macOS in ways that have bitten this app before, so verify
these specifically:

1. **`git config --global --list` still works** after creating a profile.
   Paths written into git config MUST use forward slashes — a backslash is an
   escape character there, and one bad line makes the whole `~/.gitconfig`
   unreadable by *every* git command. There is a regression test for this
   (`config_values_never_contain_backslashes`), but confirm on real hardware.
2. **Per-folder identity** — create a profile, assign a folder, then inside a
   repo in it run `git config user.email` and confirm it matches.
3. **Commit Audit / Auto Assign list something.** If they come up empty, path
   comparison is failing (`C:\Users\...` vs `/`).
4. **Doctor** — it should find `%USERPROFILE%\.ssh\config` and report sensibly.
5. **Commit guard** — enable it, then commit with a mismatched email; the hook
   is `#!/bin/sh`, which Git for Windows runs through its bundled shell.
