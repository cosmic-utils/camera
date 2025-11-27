# SPDX-License-Identifier: MPL-2.0

name := 'cosmic-camera'
export APPID := 'io.github.freddyfunk.CosmicCamera'

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

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs cargo check
cargo-check *args:
    cargo check --all-features {{args}}

# Runs a clippy check
check *args:
    cargo clippy --all-features {{args}} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Format code
fmt:
    cargo fmt

# Check code formatting (for CI)
fmt-check:
    cargo fmt --check

# Developer target: format and run
dev *args:
    cargo fmt
    just run {{args}}

# Run with debug logs
run *args:
    env RUST_LOG=cosmic_camera=info RUST_BACKTRACE=full cargo run --release {{args}}

# Run with verbose debug logs
run-debug *args:
    env RUST_LOG=cosmic_camera=debug,info RUST_BACKTRACE=full cargo run --release {{args}}

# Run tests
test *args:
    cargo test {{args}}

# Installs files
install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    install -Dm0644 {{desktop-src}} {{desktop-dst}}
    install -Dm0644 {{metainfo-src}} {{metainfo-dst}}
    for size in `ls {{icons-src}}`; do \
        install -Dm0644 "{{icons-src}}/$size/apps/{{APPID}}.svg" "{{icons-dst}}/$size/apps/{{APPID}}.svg"; \
    done

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{metainfo-dst}}
    for size in `ls {{icons-src}}`; do \
        rm -f "{{icons-dst}}/$size/apps/{{APPID}}.svg"; \
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
    # Get version from git describe
    version=$(git describe --tags --always --match "v*" 2>/dev/null || echo "unknown")
    # Strip 'v' prefix
    version="${version#v}"
    # Transform: 0.1.0-5-gabcdef1 -> 0.1.0-dirty-abcdef1
    if [[ "$version" == *-*-g* ]]; then
        base=$(echo "$version" | sed 's/-[0-9]*-g.*//')
        hash=$(echo "$version" | sed 's/.*-g//')
        version="${base}-dirty-${hash}"
    fi
    echo "$version"

# ============================================================================
# Flatpak recipes
# ============================================================================

# Generate cargo-sources.json for Flatpak
flatpak-cargo-sources:
    #!/usr/bin/env bash
    echo "Generating cargo-sources.json for Flatpak..."
    if ! command -v python3 &> /dev/null; then
        echo "Error: python3 not found!"
        exit 1
    fi
    if [ ! -f flatpak-cargo-generator.py ]; then
        echo "Downloading flatpak-cargo-generator.py..."
        curl -sLo flatpak-cargo-generator.py https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
    fi
    # Check if dependencies are available system-wide (for CI)
    if python3 -c "import aiohttp, toml" 2>/dev/null; then
        echo "Using system Python packages..."
        python3 flatpak-cargo-generator.py ./Cargo.lock -o cargo-sources.json
    else
        # Create virtual environment if it doesn't exist
        if [ ! -d .flatpak-venv ]; then
            echo "Creating virtual environment..."
            python3 -m venv .flatpak-venv
        fi
        # Install dependencies in virtual environment
        .flatpak-venv/bin/pip install --quiet aiohttp toml tomlkit
        # Run the generator
        .flatpak-venv/bin/python flatpak-cargo-generator.py ./Cargo.lock -o cargo-sources.json
    fi
    echo "Generated cargo-sources.json"

# Build and install Flatpak locally
flatpak-build: flatpak-cargo-sources
    #!/usr/bin/env bash
    echo "Building Flatpak..."
    # Generate version file for flatpak build
    just get-version > .flatpak-version
    flatpak-builder --user --install --force-clean build-dir {{APPID}}.yml
    rm -f .flatpak-version
    echo "Flatpak built and installed!"

# Build Flatpak bundle for distribution
flatpak-bundle: flatpak-cargo-sources
    #!/usr/bin/env bash
    echo "Building Flatpak bundle..."
    # Generate version file for flatpak build
    just get-version > .flatpak-version
    flatpak-builder --repo=repo --force-clean build-dir {{APPID}}.yml
    flatpak build-bundle repo {{name}}.flatpak {{APPID}}
    rm -f .flatpak-version
    echo "Flatpak bundle created: {{name}}.flatpak"

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

    # Uninstall any existing installation
    just flatpak-uninstall

    # Extract runtime version from manifest
    RUNTIME_VERSION=$(grep 'runtime-version:' {{APPID}}.yml | sed "s/.*runtime-version: *['\"]\\?\\([^'\"]*\\)['\"]\\?/\\1/")

    # Check if all dependencies are installed
    DEPS_MISSING=false
    if ! flatpak info org.freedesktop.Sdk//${RUNTIME_VERSION} &>/dev/null; then
        DEPS_MISSING=true
    fi
    if ! flatpak info org.freedesktop.Platform//${RUNTIME_VERSION} &>/dev/null; then
        DEPS_MISSING=true
    fi
    if ! flatpak info org.freedesktop.Sdk.Extension.rust-stable//${RUNTIME_VERSION} &>/dev/null; then
        DEPS_MISSING=true
    fi

    if [ "$DEPS_MISSING" = true ]; then
        echo "Flatpak dependencies missing for runtime ${RUNTIME_VERSION}, installing..."
        just flatpak-deps
    else
        echo "Flatpak dependencies already installed for runtime ${RUNTIME_VERSION}."
    fi

    # Build and install
    just flatpak-build

    echo "=== Flatpak installation complete! ==="
    echo "Run with: just flatpak-run"

# Clean Flatpak build artifacts
flatpak-clean:
    rm -rf build-dir .flatpak-builder repo cargo-sources.json {{name}}.flatpak flatpak-cargo-generator.py .flatpak-venv

# Install Flatpak dependencies (runtime and SDK)
flatpak-deps:
    #!/usr/bin/env bash
    echo "Installing Flatpak dependencies..."
    if ! command -v flatpak &> /dev/null; then
        echo "Error: flatpak not found! Please install flatpak first."
        exit 1
    fi
    # Extract runtime version from manifest
    RUNTIME_VERSION=$(grep 'runtime-version:' {{APPID}}.yml | sed "s/.*runtime-version: *['\"]\\?\\([^'\"]*\\)['\"]\\?/\\1/")
    echo "Using runtime version: $RUNTIME_VERSION"
    flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
    flatpak install -y flathub org.freedesktop.Platform//${RUNTIME_VERSION}
    flatpak install -y flathub org.freedesktop.Sdk//${RUNTIME_VERSION}
    flatpak install -y flathub org.freedesktop.Sdk.Extension.rust-stable//${RUNTIME_VERSION}
    echo "Flatpak dependencies installed!"

# Full clean (cargo + vendor + flatpak)
clean-all: clean clean-vendor flatpak-clean

# Build Flatpak bundle for a specific architecture (for CI)
flatpak-bundle-arch arch: flatpak-cargo-sources
    #!/usr/bin/env bash
    echo "Building Flatpak bundle for {{arch}}..."
    # Generate version file for flatpak build
    just get-version > .flatpak-version
    flatpak-builder --repo=repo --force-clean --arch={{arch}} build-dir {{APPID}}.yml
    flatpak build-bundle repo {{name}}-{{arch}}.flatpak {{APPID}} --arch={{arch}}
    rm -f .flatpak-version
    echo "Flatpak bundle created: {{name}}-{{arch}}.flatpak"
