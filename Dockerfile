# =============================================================================
# GitSwitch in Docker — one file, three targets:
#
#   builder (the default): the Linux installers (.deb / .AppImage / .rpm)
#       docker build -t gitswitch-builder .
#       docker run --rm -v "$PWD/dist-linux:/out" gitswitch-builder
#   app: the built app, on any Docker host, no checkout needed
#       docker compose up app        then open http://localhost:6080/vnc.html
#   dev: `tauri dev` on the checkout mounted from the host
#       docker compose up dev        then open http://localhost:6080/vnc.html
#
# A desktop app needs a display: the container runs a virtual one (Xvfb) and
# shows it in the host's browser through noVNC, bound to localhost only.
# =============================================================================
FROM rust:1-bookworm AS toolchain

# System deps required by Tauri v2 on Linux (webkit2gtk 4.1, gtk, ayatana tray…)
RUN apt-get update && apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-dev \
    libgtk-3-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    libssl-dev \
    patchelf \
    file \
    build-essential \
    curl \
    wget \
    ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Node.js 20 (for the Vite frontend build)
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
 && apt-get install -y nodejs \
 && rm -rf /var/lib/apt/lists/*

# Tauri CLI
RUN cargo install tauri-cli --version "^2" --locked

# xdg-utils provides xdg-open, required by the AppImage bundler step.
RUN apt-get update && apt-get install -y --no-install-recommends xdg-utils \
 && rm -rf /var/lib/apt/lists/*

# linuxdeploy is itself an AppImage and needs FUSE to run — containers have no
# FUSE, so tell it to self-extract instead. Required for the AppImage bundle step.
ENV APPIMAGE_EXTRACT_AND_RUN=1

WORKDIR /app

# --- the installers ------------------------------------------------------------
FROM toolchain AS compile

# Cache dependencies first
COPY package.json package-lock.json ./
RUN npm ci

COPY src-tauri/Cargo.toml src-tauri/Cargo.lock ./src-tauri/

# Now the full source
COPY . .

# Build the frontend + the Linux bundles.
# BUNDLES is comma-separated; exclude "appimage" on emulated cross-arch builds
# (linuxdeploy cannot run under QEMU/Rosetta even with extract-and-run).
ARG BUNDLES=deb,rpm,appimage
RUN npm run build \
 && cargo tauri build --bundles "$BUNDLES"

# --- `tauri dev` on the mounted checkout ---------------------------------------
FROM toolchain AS dev
RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb x11vnc novnc websockify openbox dbus dbus-x11 xauth \
    fonts-dejavu-core libgl1-mesa-dri git git-lfs openssh-client \
 && rm -rf /var/lib/apt/lists/* \
 && git config --system safe.directory '*' \
 && mkdir -p -m 1777 /tmp/.X11-unix
ENV DISPLAY=:99 WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 NO_AT_BRIDGE=1 XDG_RUNTIME_DIR=/tmp/xdg
COPY scripts/docker/display.sh scripts/docker/dev.sh /usr/local/bin/
RUN sed -i 's/\r$//' /usr/local/bin/display.sh /usr/local/bin/dev.sh \
 && chmod 755 /usr/local/bin/display.sh /usr/local/bin/dev.sh
EXPOSE 6080
CMD ["/usr/local/bin/dev.sh"]

# --- the built app, on any Docker host -----------------------------------------
FROM debian:bookworm-slim AS app
RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb x11vnc novnc websockify openbox dbus dbus-x11 xauth \
    fonts-dejavu-core libgl1-mesa-dri git git-lfs openssh-client ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && git config --system safe.directory '*' \
 && mkdir -p -m 1777 /tmp/.X11-unix
# The package pulls the webkit/gtk/tray runtime it declares, and puts the app
# and its two sidecars side by side in /usr/bin, where the app looks for them.
COPY --from=compile /app/src-tauri/target/release/bundle/deb/ /tmp/deb/
RUN apt-get update && apt-get install -y --no-install-recommends /tmp/deb/*.deb \
 && rm -rf /var/lib/apt/lists/* /tmp/deb \
 && useradd -m gitswitch
ENV DISPLAY=:99 WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 NO_AT_BRIDGE=1 XDG_RUNTIME_DIR=/tmp/xdg
COPY scripts/docker/display.sh scripts/docker/app.sh /usr/local/bin/
RUN sed -i 's/\r$//' /usr/local/bin/display.sh /usr/local/bin/app.sh \
 && chmod 755 /usr/local/bin/display.sh /usr/local/bin/app.sh
USER gitswitch
WORKDIR /home/gitswitch
EXPOSE 6080
CMD ["/usr/local/bin/app.sh"]

# --- the default: copy the installers out ----------------------------------------
FROM compile AS builder
CMD ["bash", "-c", "mkdir -p /out && cp -v src-tauri/target/release/bundle/deb/*.deb /out/ 2>/dev/null; cp -v src-tauri/target/release/bundle/appimage/*.AppImage /out/ 2>/dev/null; cp -rv src-tauri/target/release/bundle/rpm/*.rpm /out/ 2>/dev/null; echo 'Linux installers copied to ./dist-linux'"]
