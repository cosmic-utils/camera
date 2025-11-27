// SPDX-License-Identifier: MPL-2.0

use std::process::Command;

fn main() {
    // Re-run build script if git HEAD changes
    println!("cargo::rerun-if-changed=.git/HEAD");
    println!("cargo::rerun-if-changed=.git/refs/tags");

    // Check if version is already set (e.g., in flatpak builds)
    let version = if let Ok(v) = std::env::var("COSMIC_CAMERA_VERSION") {
        v
    } else {
        get_git_version()
    };

    println!("cargo::rustc-env=GIT_VERSION={}", version);
}

fn get_git_version() -> String {
    // Try to get version from git describe
    // This will return:
    // - "v0.1.0" if HEAD is exactly at a tag
    // - "v0.1.0-5-gabcdef1" if HEAD is 5 commits after v0.1.0
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--match", "v*"])
        .output();

    let version = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => {
            // Fallback: try to get just the commit hash
            get_commit_hash().unwrap_or_else(|| "unknown".to_string())
        }
    };

    // Strip 'v' prefix if present
    let version = version.strip_prefix('v').unwrap_or(&version);

    // Get the current commit hash for all builds
    let commit_hash = get_commit_hash().unwrap_or_else(|| "unknown".to_string());

    // Transform git describe output to our format:
    // "0.1.0" (exact tag) becomes "0.1.0-abcdef1"
    // "0.1.0-5-gabcdef1" (commits after tag) becomes "0.1.0-dirty-abcdef1"
    if version.contains('-') {
        // Parse: version-commits-ghash (commits after a tag)
        let parts: Vec<&str> = version.rsplitn(3, '-').collect();
        if parts.len() >= 3 {
            let hash = parts[0].strip_prefix('g').unwrap_or(parts[0]);
            let base_version = parts[2];
            format!("{}-dirty-{}", base_version, hash)
        } else {
            version.to_string()
        }
    } else {
        // Exact tag - still append commit hash for traceability
        format!("{}-{}", version, commit_hash)
    }
}

fn get_commit_hash() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}
