#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Regenerates preview/preview-0NN.png inside the pinned container image.
#
# This is the entry point for both humans and CI: the same image, the same
# compositor and the same fonts produce the same pixels, so a diff in the
# committed screenshots means the UI actually changed.
#
# Usage:
#   preview/generate-previews.sh            # regenerate all previews
#   SHOTS=001,004 preview/generate-previews.sh
#
# Environment:
#   CONTAINER_ENGINE  podman (default) or docker
#   PREVIEW_IMAGE     image tag to build and run (default camera-previews)
#   NO_BUILD_IMAGE    set to 1 to reuse an already-built image
#   SHOTS             comma-separated shot numbers, passed through
#   PREVIEW_FUZZ / PREVIEW_THRESHOLD  see sync-previews.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

ENGINE="${CONTAINER_ENGINE:-}"
if [[ -z "$ENGINE" ]]; then
    if command -v podman >/dev/null; then
        ENGINE=podman
    elif command -v docker >/dev/null; then
        ENGINE=docker
    else
        echo "error: neither podman nor docker is installed" >&2
        exit 1
    fi
fi

IMAGE="${PREVIEW_IMAGE:-camera-previews}"

# Checked here rather than inside the container, where the failure surfaces
# only as an empty preview in an otherwise valid-looking screenshot.
if [[ ! -e /dev/udmabuf ]]; then
    echo "error: /dev/udmabuf is missing. The fake camera cannot allocate buffers." >&2
    echo "       Load the module with: sudo modprobe udmabuf" >&2
    exit 1
fi

# Cargo's registry and target directory live outside the repo tree so a
# container build never fights with the host toolchain over target/.
CACHE_DIR="$REPO_DIR/.preview-cache"
mkdir -p "$CACHE_DIR/cargo" "$CACHE_DIR/target"

if [[ "${NO_BUILD_IMAGE:-0}" != "1" ]]; then
    echo "==> Building $IMAGE with $ENGINE"
    "$ENGINE" build -t "$IMAGE" -f "$SCRIPT_DIR/Containerfile" "$REPO_DIR"
fi

run_args=(
    --rm
    # libcamera's virtual pipeline exports its frame buffers through dma-buf,
    # and with no provider it fails to allocate and the app renders an empty
    # preview. /dev/udmabuf is the one providers container-accessible here;
    # /dev/dma_heap/system is root-only on Fedora. Load it with
    # `modprobe udmabuf` if the device is missing.
    #
    # SELinux denies the container access to the device node even when it is
    # passed in, so labelling has to be off for this run.
    --device /dev/udmabuf
    --security-opt label=disable
    -v "$REPO_DIR:/src:Z"
    -v "$CACHE_DIR/cargo:/cargo:Z"
    -v "$CACHE_DIR/target:/target:Z"
    -e CARGO_HOME=/cargo
    -e CARGO_TARGET_DIR=/target
    -e "SHOTS=${SHOTS:-}"
    -e "KEEP_GOING=${KEEP_GOING:-0}"
    -e "PREVIEW_FUZZ=${PREVIEW_FUZZ:-2%}"
    -e "PREVIEW_THRESHOLD=${PREVIEW_THRESHOLD:-0.005}"
)

# Rootless podman maps the container's root to the invoking user, so files land
# with the right ownership. Docker does not, so run as the caller there.
if [[ "$ENGINE" == "docker" ]]; then
    run_args+=( --user "$(id -u):$(id -g)" -e HOME=/tmp/home )
fi

echo "==> Capturing previews"
"$ENGINE" run "${run_args[@]}" "$IMAGE" \
    -c 'preview/capture-previews.sh /tmp/shots && preview/sync-previews.sh /tmp/shots preview'

echo
echo "==> Done. Review with: git diff --stat preview/"
