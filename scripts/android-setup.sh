#!/usr/bin/env bash
# Install the Android build toolchain for the Writan app (M5).
#
#   scripts/android-setup.sh                # default SDK root /opt/android-sdk
#   SDK_ROOT=$HOME/android-sdk scripts/android-setup.sh
#
# Needs sudo only for the JDK install and to create the SDK root. Everything
# else runs as your user, so a later `sdkmanager --install` works without root.
# Cloud Shell's home partition is ~5 GB: keep the SDK on / (the default) and
# point GRADLE_USER_HOME, CARGO_TARGET_DIR and $HOME/.gradle at /tmp for heavy
# builds — see docs/CLOUD.md.
#
# The versions here match app/src-tauri/gen/android and .github/workflows/
# artifacts.yml. Bump all three in the same commit when they move.
set -euo pipefail

sdk_root="${SDK_ROOT:-/opt/android-sdk}"
ndk_version="${NDK_VERSION:-27.3.13750724}"
cmdline_tools_version="${CMDLINE_TOOLS_VERSION:-13114758}"
# Gradle 9 and AGP accept 17+, but CI pins Temurin 17, so pin the same here
# instead of inheriting a newer JAVA_HOME from the shell.
java_home="${ANDROID_JAVA_HOME:-/usr/lib/jvm/java-17-openjdk-amd64}"

say() { printf '%s\n' "$*"; }

# 1. JDK 17 (the Gradle/AGP toolchain Tauri's template asks for).
if [ ! -x "$java_home/bin/java" ]; then
  say "Installing OpenJDK 17 (sudo)…"
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
    openjdk-17-jdk-headless unzip
fi

# 2. Android command-line tools: the only piece that must be fetched by hand.
sdkmanager="$sdk_root/cmdline-tools/latest/bin/sdkmanager"
if [ ! -x "$sdkmanager" ]; then
  say "Installing Android command-line tools into $sdk_root…"
  sudo mkdir -p "$sdk_root"
  sudo chown "$(id -u):$(id -g)" "$sdk_root"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL -o "$tmp/cmdline-tools.zip" \
    "https://dl.google.com/android/repository/commandlinetools-linux-${cmdline_tools_version}_latest.zip"
  unzip -q "$tmp/cmdline-tools.zip" -d "$tmp/out"
  mkdir -p "$sdk_root/cmdline-tools"
  rm -rf "$sdk_root/cmdline-tools/latest"
  mv "$tmp/out/cmdline-tools" "$sdk_root/cmdline-tools/latest"
fi

export JAVA_HOME="$java_home"
export ANDROID_HOME="$sdk_root"
export ANDROID_SDK_ROOT="$sdk_root"
export NDK_HOME="$sdk_root/ndk/$ndk_version"
export PATH="$java_home/bin:$sdk_root/cmdline-tools/latest/bin:$sdk_root/platform-tools:$PATH"

# 3. Licenses and packages. compileSdk/targetSdk 37 and the side-by-side NDK
#    come from the committed gen/android project; platform-tools is for adb.
#    (`yes` gets SIGPIPE when sdkmanager stops reading — expected, and
#    pipefail would otherwise fail the script.)
yes | sdkmanager --sdk_root="$sdk_root" --licenses >/dev/null || true
sdkmanager --sdk_root="$sdk_root" --install \
  "platform-tools" \
  "platforms;android-37.0" \
  "build-tools;37.0.0" \
  "ndk;$ndk_version"

# 4. Rust targets for the four ABIs Tauri can build.
rustup target add aarch64-linux-android armv7-linux-androideabi \
  i686-linux-android x86_64-linux-android

say ""
say "Toolchain ready. Add this to the shell that builds the app:"
say ""
say "  export JAVA_HOME=$java_home"
say "  export ANDROID_HOME=$sdk_root"
say "  export ANDROID_SDK_ROOT=$sdk_root"
say "  export NDK_HOME=$sdk_root/ndk/$ndk_version"
say "  export PATH=\"\$JAVA_HOME/bin:\$ANDROID_HOME/cmdline-tools/latest/bin:\$ANDROID_HOME/platform-tools:\$PATH\""
say ""
say "then: npm --prefix app run tauri -- android build --debug --apk --target aarch64"
