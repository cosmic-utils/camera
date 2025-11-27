// SPDX-License-Identifier: MPL-2.0

//! Integration tests for constants module

use cosmic_camera::constants::BitratePreset;

#[test]
fn test_bitrate_preset_values() {
    // Test that all presets exist (Low, Medium, High)
    assert_eq!(BitratePreset::ALL.len(), 3);
}

#[test]
fn test_bitrate_preset_ordering() {
    // Test that presets are ordered from lowest to highest quality
    let mut prev_bitrate = 0u32;
    for preset in BitratePreset::ALL {
        let bitrate = preset.bitrate_kbps(1920, 1080);
        assert!(
            bitrate >= prev_bitrate,
            "Presets should be ordered from lowest to highest"
        );
        prev_bitrate = bitrate;
    }
}

#[test]
fn test_bitrate_scales_with_resolution() {
    // Higher resolution should have higher bitrate at same preset
    let hd_bitrate = BitratePreset::Medium.bitrate_kbps(1280, 720);
    let fhd_bitrate = BitratePreset::Medium.bitrate_kbps(1920, 1080);
    let uhd_bitrate = BitratePreset::Medium.bitrate_kbps(3840, 2160);

    assert!(hd_bitrate < fhd_bitrate);
    assert!(fhd_bitrate < uhd_bitrate);
}

#[test]
fn test_bitrate_preset_display_names() {
    // Test that all presets have non-empty display names
    for preset in BitratePreset::ALL {
        let name = preset.display_name();
        assert!(
            !name.is_empty(),
            "Preset {:?} has empty display name",
            preset
        );
    }
}
