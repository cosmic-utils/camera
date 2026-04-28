#!/usr/bin/env bash
# Launch the camera app with preview source images for taking Flathub screenshots.
#
# Usage:
#   ./preview/take-screenshots.sh [path-to-camera-binary]
#
# The script launches the camera app once per source image so you can manually
# set up the required UI state and take a screenshot (e.g. with COSMIC Screenshot).
# The app opens at 900x700 automatically when --preview-source is used; phone-
# aspect shots pass `--preview-window 400x880` so they launch at that size.
#
# After taking all screenshots, rename them to preview-001.png .. preview-006.png
# and place them in the preview/ directory.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SOURCE_DIR="$SCRIPT_DIR/source"
CAMERA="${1:-$SCRIPT_DIR/../target/release/camera}"

if [[ ! -x "$CAMERA" ]]; then
    echo "Camera binary not found at: $CAMERA"
    echo "Usage: $0 [path-to-camera-binary]"
    echo "       Build first with: cargo build --release"
    exit 1
fi

if [[ ! -d "$SOURCE_DIR" ]]; then
    echo "Source images not found at: $SOURCE_DIR"
    exit 1
fi

# Each entry: "preview_number:source_file:description[:WIDTHxHEIGHT]"
# A trailing window size is optional; if omitted the app defaults to 900x700.
# Phone-aspect shots use 400x880 (≈ 1:2.2, matching modern Linux phones like
# the PinePhone Pro / Librem 5 / Volla).
SHOTS=(
    "001:0001.jpg:Photo mode with tools menu open (landscape)"
    "002:0007.jpg:Photo mode on a Linux phone:400x880"
    "003:0006.jpg:Filter picker"
    "004:0003.jpg:Video recording"
    "005:0009.jpg:QR code detection"
    "006:0008.jpg:Settings panel"
)

echo "=== Camera Preview Screenshot Helper ==="
echo ""
echo "This will launch the camera app once per preview image."
echo "For each launch:"
echo "  1. Set up the described UI state"
echo "  2. Take a screenshot (900x700 window)"
echo "  3. Close the app to continue to the next image"
echo ""

for shot in "${SHOTS[@]}"; do
    IFS=':' read -r num file desc size <<< "$shot"
    source_path="$SOURCE_DIR/$file"

    if [[ ! -f "$source_path" ]]; then
        echo "WARNING: Source image missing: $source_path — skipping preview-$num"
        continue
    fi

    echo "--- preview-$num ---"
    echo "  Source: $file"
    echo "  Action: $desc"
    if [[ -n "${size:-}" ]]; then
        echo "  Window: ${size}"
    fi
    echo ""
    read -rp "  Press Enter to launch (or 's' to skip, 'q' to quit): " choice
    case "$choice" in
        s|S) echo "  Skipped."; echo ""; continue ;;
        q|Q) echo "  Quitting."; exit 0 ;;
    esac

    if [[ -n "${size:-}" ]]; then
        "$CAMERA" --preview-source "$source_path" --preview-window "$size" || true
    else
        "$CAMERA" --preview-source "$source_path" || true
    fi
    echo ""
done

echo "=== Done ==="
echo "Rename your screenshots to preview-001.png .. preview-006.png"
echo "and place them in: $SCRIPT_DIR/"
