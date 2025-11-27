# COSMIC Camera

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
flatpak install flathub io.github.freddyfunk.CosmicCamera

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

# Run tests
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

This project is licensed under the [Mozilla Public License 2.0](LICENSE).

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.
