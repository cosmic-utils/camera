// SPDX-License-Identifier: MPL-2.0

//! Integration tests for configuration module

use cosmic_camera::Config;

#[test]
fn test_config_default() {
    // Test that default config can be created
    let config = Config::default();

    // Check sensible defaults
    assert_eq!(
        config.mirror_preview, true,
        "Mirror preview should be enabled by default"
    );
}

#[test]
fn test_config_bug_report_url() {
    // Test that bug report URL is set
    let config = Config::default();
    assert!(
        !config.bug_report_url.is_empty(),
        "Bug report URL should not be empty"
    );
}
