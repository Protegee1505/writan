# Dev-container image for the cloud-first workflow (GitHub Codespaces).
# Mirrors the apt list in .github/workflows/ci.yml so a codespace can build
# and test the same targets as CI: the Tauri desktop app and the GTK4 shell.
FROM mcr.microsoft.com/devcontainers/base:ubuntu-24.04

RUN apt-get update \
    && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        libwebkit2gtk-4.1-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        patchelf \
        libxdo-dev \
        libssl-dev \
        libgtk-4-dev \
        libadwaita-1-dev \
        libgtksourceview-5-dev \
    && rm -rf /var/lib/apt/lists/*
