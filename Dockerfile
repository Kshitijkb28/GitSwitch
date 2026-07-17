# =============================================================================
# GitSwitch — Linux package BUILDER image.
#
# IMPORTANT: This image does NOT run the app. GitSwitch is a native desktop GUI
# (Tauri), and a container has no display. This image exists to *build* the
# Linux installers (.deb / .AppImage / .rpm) reproducibly on any machine.
#
# Usage (produces installers into ./dist-linux on your host):
#   docker build -t gitswitch-builder .
#   docker run --rm -v "$PWD/dist-linux:/out" gitswitch-builder
#
# The build artifacts land in /out on the host. Nothing else is touched.
# =============================================================================
FROM rust:1-bookworm AS builder

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

WORKDIR /app

# Cache dependencies first
COPY package.json package-lock.json ./
RUN npm ci

COPY src-tauri/Cargo.toml src-tauri/Cargo.lock ./src-tauri/

# Now the full source
COPY . .

# Build the frontend + the Linux bundles
RUN npm run build \
 && cargo tauri build

# Default: copy the built installers to the mounted /out volume.
CMD ["bash", "-c", "mkdir -p /out && cp -v src-tauri/target/release/bundle/deb/*.deb /out/ 2>/dev/null; cp -v src-tauri/target/release/bundle/appimage/*.AppImage /out/ 2>/dev/null; cp -rv src-tauri/target/release/bundle/rpm/*.rpm /out/ 2>/dev/null; echo 'Linux installers copied to ./dist-linux'"]
