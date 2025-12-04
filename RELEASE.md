# Release Guide

This document describes how to create a new release of Camera.

## Overview

The release process uses two GitHub Actions workflows:

1. **Create Release** (`create-release.yml`) - Manually triggered to prepare and tag a release
2. **Release** (`release.yml`) - Automatically triggered when a tag is pushed, builds and publishes the release

## Creating a New Release

### Step 1: Go to GitHub Actions

Navigate to the repository's **Actions** tab and select the **"Create Release"** workflow from the left sidebar.

### Step 2: Run the Workflow

Click **"Run workflow"** and you'll see an input field:

| Field | Description |
|-------|-------------|
| **Version number** | Optional. Leave empty to auto-increment the patch version (e.g., `0.1.6` → `0.1.7`). Or specify a version like `0.2.0` or `1.0.0`. |

Click **"Run workflow"** to start.

### Step 3: What Happens Automatically

The workflow will:

1. **Determine the version** - Uses your input or auto-increments from the latest tag
2. **Generate release notes** - Fetches "What's Changed" from merged PRs since the last release
3. **Update metainfo.xml** - Adds a new `<release>` entry with the changelog
4. **Commit the change** - Pushes the updated metainfo to the main branch
5. **Create the git tag** - Tags the commit with the new version (e.g., `v0.1.7`)

This triggers the **Release** workflow, which:

1. **Builds binaries** - For x86_64, aarch64, and riscv64
2. **Builds Flatpak bundles** - For x86_64 and aarch64
3. **Creates GitHub Release** - With all artifacts and release notes
4. **Publishes to Flathub** - Updates the Flathub repository (if `FLATHUB_TOKEN` is configured)

## Version Numbering

The project follows [Semantic Versioning](https://semver.org/):

- **MAJOR** (x.0.0) - Incompatible API/behavior changes
- **MINOR** (0.x.0) - New features, backward compatible
- **PATCH** (0.0.x) - Bug fixes, backward compatible

Examples:
- `0.1.7` - Patch release with bug fixes
- `0.2.0` - Minor release with new features
- `1.0.0` - Major stable release

## Prerequisites

### For GitHub Releases
No additional setup required. The workflow uses the default `GITHUB_TOKEN`.

### For Flathub Publishing
The `FLATHUB_TOKEN` secret must be configured in the repository settings:

1. Create a GitHub Personal Access Token with `repo` scope
2. The token must have write access to the [flathub/io.github.cosmic_utils.camera](https://github.com/flathub/io.github.cosmic_utils.camera) repository
3. Add it as a repository secret named `FLATHUB_TOKEN`

If the secret is not set, the Flathub publishing step will be skipped.

## Troubleshooting

### "Tag already exists" Error
The workflow will fail if you try to create a version that already exists. Check existing tags:
```bash
git tag --sort=-v:refname | head -10
```

### Release Notes Are Empty
Release notes are generated from merged pull requests. If there are no PRs since the last release, a default message "Bug fixes and improvements." will be used.

### Flathub Build Fails
If the Flathub build fails after publishing:
1. Check the [Flathub Buildbot](https://buildbot.flathub.org/#/apps/io.github.cosmic_utils.camera)
2. Common issues: missing dependencies, cargo source hash mismatches
3. You may need to manually fix the Flathub repository

## Manual Release (Emergency)

If the automated workflow fails, you can create a release manually:

```bash
# 1. Update metainfo.xml manually with the new version and date
# 2. Commit the change
git add resources/io.github.cosmic_utils.camera.metainfo.xml
git commit -m "Release v0.1.7: update metainfo.xml"
git push origin main

# 3. Create and push the tag
git tag -a v0.1.7 -m "Release v0.1.7"
git push origin v0.1.7
```

This will trigger the Release workflow to build and publish.

## Release Artifacts

Each release includes:

| Artifact | Description |
|----------|-------------|
| `camera-x86_64-linux.tar.gz` | Linux binary for x86_64 |
| `camera-aarch64-linux.tar.gz` | Linux binary for ARM64 |
| `camera-riscv64-linux.tar.gz` | Linux binary for RISC-V 64 |
| `camera-x86_64.flatpak` | Flatpak bundle for x86_64 |
| `camera-aarch64.flatpak` | Flatpak bundle for ARM64 |
| `camera-vX.Y.Z-source.zip` | Source code archive |
