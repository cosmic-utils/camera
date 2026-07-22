#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Copies freshly captured screenshots over the committed ones, but only where
# they actually differ.
#
# Two captures of an unchanged UI are not guaranteed to be byte-identical: a
# blinking recording dot, a timer reading 00:01 instead of 00:02, a pixel of
# antialiasing. Committing those would open a pull request on every push and
# train everyone to ignore preview PRs. A shot is only replaced when a
# meaningful share of its pixels changed.
#
# That share is measured two ways, and either one accepts the shot:
#
#   whole-frame  the share of differing pixels across the entire image, which
#                catches broad changes (a theme, a relaid-out panel)
#   worst tile   the same share computed over a 16x16 grid, keeping the worst
#                cell, which catches a small element that is entirely wrong
#
# The tile pass exists because the whole-frame share alone is a function of how
# *large* the changed element is, not how wrong it is. A corrupt gallery
# thumbnail covers about 0.25% of a 900x700 shot, under any whole-frame
# threshold loose enough to absorb a blinking dot, so a screenshot showing a
# green-smeared thumbnail was silently kept over a correct fresh capture. Within
# its own tile that same thumbnail is a ~40% change, far above the timer digit
# or antialiasing it has to be told apart from.
#
# Usage:
#   preview/sync-previews.sh <new-dir> <committed-dir>
#
# Environment:
#   PREVIEW_FUZZ        per-pixel colour tolerance, ImageMagick syntax (default 2%)
#   PREVIEW_THRESHOLD   share of differing pixels needed to accept a shot (default 0.005 = 0.5%)
#   PREVIEW_TILE_THRESHOLD
#                       share of differing pixels within the worst grid cell
#                       needed to accept a shot (default 0.25 = 25%)
#   PREVIEW_TILES       grid resolution for the tile pass (default 16)

set -euo pipefail

NEW_DIR="${1:?usage: sync-previews.sh <new-dir> <committed-dir>}"
OLD_DIR="${2:?usage: sync-previews.sh <new-dir> <committed-dir>}"

FUZZ="${PREVIEW_FUZZ:-2%}"
THRESHOLD="${PREVIEW_THRESHOLD:-0.005}"
TILE_THRESHOLD="${PREVIEW_TILE_THRESHOLD:-0.25}"
TILES="${PREVIEW_TILES:-16}"

command -v magick >/dev/null || command -v convert >/dev/null || {
    echo "error: ImageMagick is required" >&2
    exit 1
}

# ImageMagick 7 folded the tools into one `magick` command; support both.
if command -v magick >/dev/null; then
    im_convert() { magick "$@"; }
    im_identify() { magick identify "$@"; }
else
    im_convert() { convert "$@"; }
    im_identify() { identify "$@"; }
fi

# Prints the share of pixels (0..1) that differ by more than FUZZ.
#
# Deliberately not `compare -metric AE`: on an HDRI build that returns a
# scaled error sum in scientific notation rather than a pixel count, which a
# previous version of this script parsed as a small integer and reported every
# screenshot as unchanged, including ones that were entirely wrong.
#
# Instead: absolute per-channel difference, flattened to grey, thresholded at
# FUZZ. Every pixel is then black (within tolerance) or white (differs), so the
# mean of the result *is* the share of differing pixels.
diff_share() {
    im_convert "$1" "$2" -compose Difference -composite \
        -colorspace Gray -threshold "$FUZZ" -format '%[fx:mean]' info:
}

# Prints the share of differing pixels (0..1) within the worst cell of a
# TILES x TILES grid.
#
# `-scale` to a fixed grid averages each cell, so on the same black/white
# thresholded diff every output pixel is that cell's share of differing pixels
# and the maximum is the worst cell. `!` forces the exact grid regardless of
# aspect ratio.
diff_tile_share() {
    im_convert "$1" "$2" -compose Difference -composite \
        -colorspace Gray -threshold "$FUZZ" -scale "${TILES}x${TILES}!" \
        -format '%[fx:maxima]' info:
}

# A comparison that fails must never look like "nothing changed": that is how
# a broken harness quietly keeps stale screenshots. The value may be in
# scientific notation (1.5873e-06 is a single differing pixel), so accept that
# spelling; awk parses it correctly.
require_share() {
    if ! [[ "$1" =~ ^[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?$ ]]; then
        echo "error: could not compare $2 (got '$1')" >&2
        exit 1
    fi
}

as_pct() { awk "BEGIN { printf \"%.2f\", $1 * 100 }"; }

changed=0
unchanged=0
added=0

shopt -s nullglob
# The published English shots sit at the top level, the appearance variants under
# variants/, and one published shot per translated language under
# locales/<lang>/. All are synced the same way, so neither the gallery in
# preview/README.md nor the localized screenshots in metainfo.xml drift from the
# shot they illustrate.
for new in "$NEW_DIR"/preview-*.png "$NEW_DIR"/variants/preview-*.png \
           "$NEW_DIR"/locales/*/preview-*.png; do
    name="${new#"$NEW_DIR"/}"
    old="$OLD_DIR/$name"
    mkdir -p "$(dirname "$old")"

    if [[ ! -f "$old" ]]; then
        cp "$new" "$old"
        echo "added    $name (no committed version)"
        added=$(( added + 1 ))
        continue
    fi

    new_size="$(im_identify -format '%wx%h' "$new")"
    old_size="$(im_identify -format '%wx%h' "$old")"
    if [[ "$new_size" != "$old_size" ]]; then
        cp "$new" "$old"
        echo "changed  $name (size ${old_size} -> ${new_size})"
        changed=$(( changed + 1 ))
        continue
    fi

    share="$(diff_share "$old" "$new")"
    require_share "$share" "$name"

    tile_share="$(diff_tile_share "$old" "$new")"
    require_share "$tile_share" "$name"

    if awk "BEGIN { exit !($share > $THRESHOLD) }"; then
        cp "$new" "$old"
        printf 'changed  %s (%s%% of pixels)\n' "$name" "$(as_pct "$share")"
        changed=$(( changed + 1 ))
    elif awk "BEGIN { exit !($tile_share > $TILE_THRESHOLD) }"; then
        cp "$new" "$old"
        printf 'changed  %s (%s%% of pixels overall, but %s%% within one tile)\n' \
            "$name" "$(as_pct "$share")" "$(as_pct "$tile_share")"
        changed=$(( changed + 1 ))
    else
        printf 'unchanged %s (%s%% of pixels, %s%% worst tile, below thresholds)\n' \
            "$name" "$(as_pct "$share")" "$(as_pct "$tile_share")"
        unchanged=$(( unchanged + 1 ))
    fi
done

echo
printf '%s changed, %s added, %s unchanged (fuzz %s, threshold %s%%, tile threshold %s%% on a %sx%s grid)\n' \
    "$changed" "$added" "$unchanged" "$FUZZ" "$(as_pct "$THRESHOLD")" "$(as_pct "$TILE_THRESHOLD")" "$TILES" "$TILES"
