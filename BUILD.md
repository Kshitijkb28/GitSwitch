# Building & Installing GitSwitch

GitSwitch is a **native desktop app** (Tauri v2 = Rust backend + web UI). One
codebase builds installers for **Windows, macOS, and Linux**.

## Common commands (cheat sheet)

```bash
# Run the full app from source (dev, hot-reload) ...... npm run tauri dev   [native]
# Build a production app for THIS machine's OS ........ npm run tauri build [native]
# Build Linux installers on any machine ............... docker compose run --rm builder
# Build ALL three OSes' installers .................... git tag vX.Y.Z && git push --tags  (CI)
```

> To **run** GitSwitch from the codebase, use `npm run tauri dev` — Docker is a
> *build* tool here, not a run tool (a container has no display for a GUI app).

> **Docker note:** Docker cannot *run* GitSwitch — a container has no display,
> and this is a GUI app. Docker is only used here to **build the Linux
> installer** reproducibly. See [Linux via Docker](#linux-via-docker).

---

## 1. Install a released build (end users)

Download the installer for your OS from the project's GitHub **Releases** page
and run it:

| OS | File | How to install |
|----|------|----------------|
| **macOS** | `GitSwitch_x.y.z_universal.dmg` | Open the `.dmg`, drag GitSwitch to Applications |
| **Windows** | `GitSwitch_x.y.z_x64-setup.exe` or `.msi` | Run it, click through the installer |
| **Linux (Debian/Ubuntu)** | `git-switch_x.y.z_amd64.deb` | `sudo dpkg -i *.deb` |
| **Linux (any distro)** | `git-switch_x.y.z_amd64.AppImage` | `chmod +x *.AppImage && ./*.AppImage` |
| **Linux (Fedora/RHEL)** | `git-switch-x.y.z.x86_64.rpm` | `sudo rpm -i *.rpm` |

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
(Needs the "Desktop development with C++" workload from Visual Studio Build Tools.)

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
> packages; on Intel/AMD machines you get **x86_64**. To force a specific arch,
> add `platform: linux/amd64` (or `linux/arm64`) under the `builder` service in
> `docker-compose.yml` — cross-arch builds run under emulation and are slower.
> For clean native builds of both arches, prefer the GitHub Actions CI below.

---

## 3. Release all three OSes at once (CI)

`.github/workflows/build.yml` builds macOS + Windows + Linux installers in
parallel. Two ways to trigger it:

- **Tag a release:** `git tag v0.1.0 && git push --tags` → a draft GitHub
  Release is created with every installer attached.
- **Manual:** Actions tab → "Build installers" → Run workflow → download the
  per-OS artifacts.

This is the recommended path, because macOS installers can only be built on
macOS and Windows installers on Windows.
