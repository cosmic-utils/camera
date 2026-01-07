# SPDX-License-Identifier: GPL-3.0-only

name := 'camera'
export APPID := 'io.github.cosmic_utils.camera'

rootdir := ''
prefix := '/usr'

base-dir := absolute_path(clean(rootdir / prefix))

export INSTALL_DIR := base-dir / 'share'

cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
bin-src := cargo-target-dir / 'release' / name
bin-dst := base-dir / 'bin' / name

desktop := APPID + '.desktop'
desktop-src := 'resources' / desktop
desktop-dst := clean(rootdir / prefix) / 'share' / 'applications' / desktop

metainfo := APPID + '.metainfo.xml'
metainfo-src := 'resources' / metainfo
metainfo-dst := clean(rootdir / prefix) / 'share' / 'metainfo' / metainfo

icons-src := 'resources' / 'icons' / 'hicolor'
icons-dst := clean(rootdir / prefix) / 'share' / 'icons' / 'hicolor'

# Default recipe which runs `just build-release`
default: build-release

# ============================================================================
# Building
# ============================================================================

# Compiles with debug profile
build-debug *args:
    cargo build {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# ============================================================================
# Code quality
# ============================================================================

# Runs cargo check
cargo-check *args:
    cargo check --all-features {{args}}

# Runs clippy
clippy *args:
    cargo clippy --all-features {{args}} -- -W clippy::pedantic

# Runs clippy with JSON message format
clippy-json: (clippy '--message-format=json')

# Format code
fmt:
    cargo fmt

# Check code formatting
fmt-check:
    cargo fmt --check

# Run tests
test *args:
    cargo test {{args}}

# Run all checks (format, cargo check, test)
check: fmt-check cargo-check test

# ============================================================================
# Development
# ============================================================================

# Developer target: format and run
dev *args:
    cargo fmt
    just run {{args}}

# Run with debug logs
run *args:
    env RUST_LOG=camera=info RUST_BACKTRACE=full cargo run --release {{args}}

# Run with verbose debug logs
run-debug *args:
    env RUST_LOG=camera=debug,info RUST_BACKTRACE=full cargo run --release {{args}}

# ============================================================================
# Resource generation
# ============================================================================

# Generate PNG icons from the scalable SVG and update desktop file
generate-icons:
    #!/usr/bin/env bash
    set -euo pipefail
    SVG="{{icons-src}}/scalable/apps/{{APPID}}.svg"
    DESKTOP="{{desktop-src}}"
    if [ ! -f "$SVG" ]; then
        echo "Error: Scalable SVG not found at $SVG"
        exit 1
    fi
    # Generate PNG icons for each size
    for size in 16 24 32 48 64 128 256; do
        DIR="{{icons-src}}/${size}x${size}/apps"
        mkdir -p "$DIR"
        magick -background none -density 384 "$SVG" -resize ${size}x${size} "$DIR/{{APPID}}.png"
        echo "Generated ${size}x${size} icon"
    done
    # Update Icon= line in desktop file
    if [ -f "$DESKTOP" ]; then
        sed -i 's/^Icon=.*/Icon={{APPID}}/' "$DESKTOP"
        echo "Updated desktop file icon to {{APPID}}"
    else
        echo "Warning: Desktop file not found at $DESKTOP"
    fi
    echo "All icons generated successfully!"

# ============================================================================
# Cleaning
# ============================================================================

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Installs files
install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    install -Dm0644 {{desktop-src}} {{desktop-dst}}
    install -Dm0644 {{metainfo-src}} {{metainfo-dst}}
    install -Dm0644 "{{icons-src}}/scalable/apps/{{APPID}}.svg" "{{icons-dst}}/scalable/apps/{{APPID}}.svg"
    for size in 16x16 24x24 32x32 48x48 64x64 128x128 256x256; do \
        install -Dm0644 "{{icons-src}}/$size/apps/{{APPID}}.png" "{{icons-dst}}/$size/apps/{{APPID}}.png"; \
    done

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{metainfo-dst}}
    rm -f "{{icons-dst}}/scalable/apps/{{APPID}}.svg"
    for size in 16x16 24x24 32x32 48x48 64x64 128x128 256x256; do \
        rm -f "{{icons-dst}}/$size/apps/{{APPID}}.png"; \
    done

# Vendor dependencies locally
vendor:
    #!/usr/bin/env bash
    mkdir -p .cargo
    cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    echo >> .cargo/config.toml
    echo '[env]' >> .cargo/config.toml
    if [ -n "${SOURCE_DATE_EPOCH}" ]
    then
        source_date="$(date -d "@${SOURCE_DATE_EPOCH}" "+%Y-%m-%d")"
        echo "VERGEN_GIT_COMMIT_DATE = \"${source_date}\"" >> .cargo/config.toml
    fi
    if [ -n "${SOURCE_GIT_HASH}" ]
    then
        echo "VERGEN_GIT_SHA = \"${SOURCE_GIT_HASH}\"" >> .cargo/config.toml
    fi
    tar pcf vendor.tar .cargo vendor
    rm -rf .cargo vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# ============================================================================
# Version management
# ============================================================================

# Get the current version from git tags
get-version:
    #!/usr/bin/env bash
    version=$(git describe --tags --always --match "v*" 2>/dev/null || echo "unknown")
    version="${version#v}"
    # Transform: 0.1.0-5-gabcdef1 -> 0.1.0-dirty-abcdef1
    if [[ "$version" == *-*-g* ]]; then
        base=$(echo "$version" | sed 's/-[0-9]*-g.*//')
        hash=$(echo "$version" | sed 's/.*-g//')
        version="${base}-dirty-${hash}"
    fi
    echo "$version"

# Get runtime version from Flatpak manifest
[private]
flatpak-runtime-version:
    @grep 'runtime-version:' {{APPID}}.yml | sed "s/.*runtime-version: *['\"]\\?\\([^'\"]*\\)['\"]\\?/\\1/"

# ============================================================================
# Flatpak recipes
# ============================================================================

# Generate cargo-sources.json for Flatpak
flatpak-cargo-sources:
    #!/usr/bin/env bash
    echo "Generating cargo-sources.json..."
    if ! command -v python3 &> /dev/null; then
        echo "Error: python3 not found!"
        exit 1
    fi
    [ -f flatpak-cargo-generator.py ] || curl -sLo flatpak-cargo-generator.py \
        https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
    if python3 -c "import aiohttp, toml" 2>/dev/null; then
        python3 flatpak-cargo-generator.py ./Cargo.lock -o cargo-sources.json
    else
        [ -d .flatpak-venv ] || python3 -m venv .flatpak-venv
        .flatpak-venv/bin/pip install --quiet aiohttp toml tomlkit
        .flatpak-venv/bin/python flatpak-cargo-generator.py ./Cargo.lock -o cargo-sources.json
    fi
    echo "Generated cargo-sources.json"

# Build and install Flatpak locally
flatpak-build: flatpak-cargo-sources
    #!/usr/bin/env bash
    echo "Building Flatpak..."
    just get-version > .flatpak-version
    flatpak-builder --user --install --force-clean build-dir {{APPID}}.yml
    rm -f .flatpak-version
    echo "Flatpak built and installed!"

# Build Flatpak bundle for distribution (optionally specify arch)
flatpak-bundle arch="": flatpak-cargo-sources
    #!/usr/bin/env bash
    set -euo pipefail
    arch="{{arch}}"
    if [ -z "$arch" ]; then
        arch=$(uname -m)
        [ "$arch" = "x86_64" ] || [ "$arch" = "aarch64" ] || { echo "Unknown arch: $arch"; exit 1; }
    fi
    echo "Building Flatpak bundle for $arch..."
    just get-version > .flatpak-version
    flatpak-builder --repo=repo --force-clean --arch=$arch build-dir {{APPID}}.yml
    flatpak build-bundle repo {{name}}-$arch.flatpak {{APPID}} --arch=$arch
    rm -f .flatpak-version
    [ -f "{{name}}-$arch.flatpak" ] || { echo "Error: Flatpak bundle was not created!"; exit 1; }
    echo "Flatpak bundle created: {{name}}-$arch.flatpak"
    ls -la {{name}}-$arch.flatpak

# Run the installed Flatpak
flatpak-run:
    flatpak run {{APPID}}

# Uninstall all Flatpak components (app, debug, locale)
flatpak-uninstall:
    #!/usr/bin/env bash
    echo "Uninstalling all {{APPID}} Flatpak components..."
    flatpak uninstall --user -y {{APPID}} 2>/dev/null || true
    flatpak uninstall --user -y {{APPID}}.Debug 2>/dev/null || true
    flatpak uninstall --user -y {{APPID}}.Locale 2>/dev/null || true
    echo "Flatpak uninstalled!"

# Full Flatpak install: uninstall old, install deps if needed, build, and install
flatpak-install:
    #!/usr/bin/env bash
    set -e
    echo "=== Full Flatpak Install ==="
    just flatpak-uninstall
    RUNTIME_VERSION=$(just flatpak-runtime-version)
    DEPS_MISSING=false
    flatpak info org.freedesktop.Sdk//${RUNTIME_VERSION} &>/dev/null || DEPS_MISSING=true
    flatpak info org.freedesktop.Platform//${RUNTIME_VERSION} &>/dev/null || DEPS_MISSING=true
    flatpak info com.system76.Cosmic.BaseApp//stable &>/dev/null || DEPS_MISSING=true
    if [ "$DEPS_MISSING" = true ]; then
        echo "Flatpak dependencies missing, installing..."
        just flatpak-deps
    else
        echo "Flatpak dependencies already installed."
    fi
    just flatpak-build
    echo "=== Flatpak installation complete! ==="
    echo "Run with: just flatpak-run"

# Clean Flatpak build artifacts
flatpak-clean:
    rm -rf build-dir .flatpak-builder repo cargo-sources.json {{name}}-*.flatpak flatpak-cargo-generator.py .flatpak-venv .flatpak-version

# Install Flatpak dependencies (runtime and SDK)
flatpak-deps arch="":
    #!/usr/bin/env bash
    echo "Installing Flatpak dependencies..."
    command -v flatpak &> /dev/null || { echo "Error: flatpak not found!"; exit 1; }
    RUNTIME_VERSION=$(just flatpak-runtime-version)
    ARCH_FLAG=""
    [ -n "{{arch}}" ] && ARCH_FLAG="--arch={{arch}}"
    echo "Runtime version: $RUNTIME_VERSION"
    flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
    sudo flatpak install -y --noninteractive flathub org.freedesktop.Platform//${RUNTIME_VERSION} $ARCH_FLAG
    sudo flatpak install -y --noninteractive flathub org.freedesktop.Sdk//${RUNTIME_VERSION} $ARCH_FLAG
    sudo flatpak install -y --noninteractive flathub org.freedesktop.Sdk.Extension.rust-stable//${RUNTIME_VERSION} $ARCH_FLAG
    sudo flatpak install -y --noninteractive flathub com.system76.Cosmic.BaseApp//stable $ARCH_FLAG
    echo "Flatpak dependencies installed!"

# Full clean (cargo + vendor + flatpak)
clean-all: clean clean-vendor flatpak-clean
