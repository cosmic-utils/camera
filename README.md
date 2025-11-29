# COSMIC Camera

[![Flathub](https://img.shields.io/flathub/v/io.github.freddyfunk.cosmic-camera?logo=flathub&logoColor=white)](https://flathub.org/apps/io.github.freddyfunk.cosmic-camera)
[![CI](https://github.com/FreddyFunk/cosmic-camera/actions/workflows/ci.yml/badge.svg)](https://github.com/FreddyFunk/cosmic-camera/actions/workflows/ci.yml)
[![Release](https://github.com/FreddyFunk/cosmic-camera/actions/workflows/release.yml/badge.svg)](https://github.com/FreddyFunk/cosmic-camera/actions/workflows/release.yml)

A camera application for the [COSMIC](https://github.com/pop-os/cosmic-epoch) desktop environment.

![COSMIC Camera Preview](preview/preview-001.png)

[View more screenshots](preview/README.md)

## Status

This is a personal project by [Frederic Laing](https://github.com/FreddyFunk). It is not affiliated with or endorsed by System76. The application may be contributed to System76 or the COSMIC project in the future if there is interest.

## Features

- Take high-quality photos
- Record videos with audio
- Support for multiple cameras via PipeWire
- Various resolution and format options
- Theatre mode for fullscreen preview
- Hardware-accelerated video encoding (VA-API, NVENC, QuickSync)
- Gallery view for captured media

## Installation

### Flatpak (Recommended)

```bash
# Install from Flathub (when available)
flatpak install flathub io.github.freddyfunk.cosmic-camera

# Or install from a downloaded .flatpak bundle
flatpak install cosmic-camera-x86_64.flatpak
```

### From Source

#### Dependencies

- Rust (stable)
- GStreamer 1.0 with plugins (base, good, bad, ugly)
- libwayland
- libxkbcommon
- libinput
- libudev
- libseat

#### Build

```bash
# Install just command runner
cargo install just

# Build release binary
just build-release

# Install to system
sudo just install
```

## Development

```bash
# Run with debug logging
just run

# Run with verbose debug logging
just run-debug

# Format code
just fmt

# Run all checks (format, cargo check, tests)
just check

# Run clippy lints
just clippy

# Run tests only
just test
```

### Flatpak Development

```bash
# Full install (uninstalls old, installs deps if needed, builds and installs)
just flatpak-install

# Run the installed Flatpak
just flatpak-run

# Uninstall all Flatpak components
just flatpak-uninstall

# Individual steps (if needed)
just flatpak-deps   # Install Flatpak SDK/runtime
just flatpak-build  # Build and install Flatpak
just flatpak-clean  # Remove build artifacts
```

## License

Licensed under the [GNU Public License 3.0](https://choosealicense.com/licenses/gpl-3.0).

### Contribution

Any contribution intentionally submitted for inclusion in the work by you shall be licensed under the GNU Public License 3.0 (GPL-3.0). Each source file should have a SPDX copyright notice at the top of the file:

```
// SPDX-License-Identifier: GPL-3.0-only
```

### Reporting Bugs

The easiest way to report a bug is to use the **"Report a Bug"** button in the app settings. This generates a detailed system report that helps with debugging.

1. Open COSMIC Camera → Settings → "Report a Bug"
2. A bug report file will be saved to `~/Pictures/cosmic-camera/`
3. Your browser will open the [bug report form](https://github.com/FreddyFunk/cosmic-camera/issues/new?template=bug_report_from_app.yml)
4. Attach the generated report file and describe the issue

You can also [report bugs manually](https://github.com/FreddyFunk/cosmic-camera/issues/new?template=bug_report.yml) if you prefer.

### Feature Requests

Have an idea for a new feature? [Submit a feature request](https://github.com/FreddyFunk/cosmic-camera/issues/new?template=feature_request.yml)!
