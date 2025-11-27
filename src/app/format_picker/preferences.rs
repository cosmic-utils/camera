// SPDX-License-Identifier: MPL-2.0

//! Format selection and preference logic

use crate::backends::camera::types::CameraFormat;
use crate::media::Codec;
use tracing::info;

/// Select format with maximum resolution (for Photo mode)
///
/// Photo mode: ALWAYS select maximum resolution, regardless of codec.
/// The user wants the highest quality photo possible.
///
/// Codec preference when multiple formats have same resolution:
/// Raw formats: YUYV > UYVY > YUY2 > NV12 > YV12 > I420
/// Encoded formats: H.264 > HW-accelerated MJPEG > HW-accelerated > MJPEG > First
pub fn select_max_resolution_format(formats: &[CameraFormat]) -> Option<CameraFormat> {
    if formats.is_empty() {
        return None;
    }

    info!("Photo mode: selecting maximum resolution (any codec)");

    // Find max resolution (by total pixels)
    let max_pixels = formats.iter().map(|f| f.width * f.height).max()?;

    // Get all formats with max resolution
    let max_res_formats: Vec<_> = formats
        .iter()
        .filter(|f| f.width * f.height == max_pixels)
        .cloned()
        .collect();

    // Apply codec preference to filtered formats
    select_best_codec(&max_res_formats)
}

/// Select best codec/pixel format from a list of formats
/// Preference order: Raw > H.264 > HW-accelerated MJPEG > HW-accelerated > MJPEG > First
pub fn select_best_codec(formats: &[CameraFormat]) -> Option<CameraFormat> {
    formats
        .iter()
        .find(|f| is_raw_format(&f.pixel_format))
        .or_else(|| formats.iter().find(|f| f.pixel_format == "H264"))
        .or_else(|| {
            formats
                .iter()
                .find(|f| f.hardware_accelerated && f.pixel_format == "MJPG")
        })
        .or_else(|| formats.iter().find(|f| f.hardware_accelerated))
        .or_else(|| formats.iter().find(|f| f.pixel_format == "MJPG"))
        .or_else(|| formats.first())
        .cloned()
}

/// Check if a pixel format is raw/uncompressed
pub fn is_raw_format(pixel_format: &str) -> bool {
    Codec::from_fourcc(pixel_format).is_raw()
}

/// Select format for first-time video mode usage
/// Selects highest resolution with at least 25 fps, preferring highest framerate up to 60 fps
pub fn select_first_time_video_format(formats: &[CameraFormat]) -> Option<CameraFormat> {
    use std::collections::HashMap;

    // Group formats by resolution
    let mut resolution_groups: HashMap<(u32, u32), Vec<&CameraFormat>> = HashMap::new();
    for format in formats {
        if let Some(fps) = format.framerate {
            // Only consider formats with at least 25 fps
            if fps >= 25 {
                resolution_groups
                    .entry((format.width, format.height))
                    .or_insert_with(Vec::new)
                    .push(format);
            }
        }
    }

    if resolution_groups.is_empty() {
        // No formats with >= 25 fps, fall back to any format
        return formats.first().cloned();
    }

    // Find the highest resolution (by pixel count)
    let highest_resolution = resolution_groups
        .keys()
        .max_by_key(|(w, h)| w * h)
        .copied()?;

    // Get formats for the highest resolution
    let formats_at_highest_res = resolution_groups.get(&highest_resolution)?;

    // Among these, find the one with the best framerate:
    // - Prefer 60 fps if available
    // - Otherwise, highest fps <= 60
    // - If all fps > 60, pick the one closest to 60
    let best_format = formats_at_highest_res
        .iter()
        .filter_map(|f| f.framerate.map(|fps| (f, fps)))
        .min_by_key(|(_, fps)| {
            if *fps == 60 {
                0 // Perfect match - highest priority
            } else if *fps < 60 {
                60 - fps // Prefer higher fps below 60
            } else {
                fps - 60 + 1000 // Deprioritize fps > 60, but still consider them
            }
        })
        .map(|(f, _)| *f)
        .cloned();

    best_format.or_else(|| formats.first().cloned())
}

/// Find a format matching specific criteria
pub fn find_format_with_criteria<F>(formats: &[CameraFormat], filter: F) -> Option<CameraFormat>
where
    F: Fn(&CameraFormat) -> bool,
{
    formats.iter().find(|f| filter(f)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_format(
        width: u32,
        height: u32,
        pixel_format: &str,
        hw_accel: bool,
    ) -> CameraFormat {
        CameraFormat {
            width,
            height,
            framerate: Some(30),
            hardware_accelerated: hw_accel,
            pixel_format: pixel_format.to_string(),
        }
    }

    #[test]
    fn test_is_raw_format() {
        assert!(is_raw_format("YUYV"));
        assert!(is_raw_format("UYVY"));
        assert!(is_raw_format("NV12"));
        assert!(!is_raw_format("MJPG"));
        assert!(!is_raw_format("H264"));
    }

    #[test]
    fn test_select_best_codec_prefers_raw() {
        let formats = vec![
            create_test_format(1920, 1080, "MJPG", true),
            create_test_format(1920, 1080, "YUYV", false),
            create_test_format(1920, 1080, "H264", false),
        ];

        let best = select_best_codec(&formats).unwrap();
        assert_eq!(best.pixel_format, "YUYV");
    }

    #[test]
    fn test_select_best_codec_prefers_h264_over_mjpeg() {
        let formats = vec![
            create_test_format(1920, 1080, "MJPG", true),
            create_test_format(1920, 1080, "H264", false),
        ];

        let best = select_best_codec(&formats).unwrap();
        assert_eq!(best.pixel_format, "H264");
    }

    #[test]
    fn test_select_best_codec_prefers_hw_accelerated_mjpeg() {
        let formats = vec![
            create_test_format(1920, 1080, "MJPG", false),
            create_test_format(1920, 1080, "MJPG", true),
        ];

        let best = select_best_codec(&formats).unwrap();
        assert!(best.hardware_accelerated);
    }

    #[test]
    fn test_select_max_resolution_format() {
        let formats = vec![
            create_test_format(1920, 1080, "YUYV", false),
            create_test_format(1280, 720, "YUYV", false),
            create_test_format(3840, 2160, "MJPG", true),
        ];

        let max_res = select_max_resolution_format(&formats).unwrap();
        // Photo mode: ALWAYS select maximum resolution, regardless of codec
        // This gives the highest quality photo possible
        assert_eq!(max_res.width, 3840);
        assert_eq!(max_res.height, 2160);
        assert_eq!(max_res.pixel_format, "MJPG");
    }

    #[test]
    fn test_find_format_with_criteria() {
        let formats = vec![
            create_test_format(1920, 1080, "YUYV", false),
            create_test_format(1920, 1080, "MJPG", true),
            create_test_format(1280, 720, "YUYV", false),
        ];

        let found =
            find_format_with_criteria(&formats, |f| f.width == 1920 && f.pixel_format == "MJPG");

        assert!(found.is_some());
        let fmt = found.unwrap();
        assert_eq!(fmt.width, 1920);
        assert_eq!(fmt.pixel_format, "MJPG");
    }

    #[test]
    fn test_select_format_empty_returns_none() {
        let formats: Vec<CameraFormat> = vec![];
        assert!(select_max_resolution_format(&formats).is_none());
    }

    #[test]
    fn test_select_format_picks_highest_resolution_for_photo() {
        // Photo mode always picks highest resolution for best photo quality
        let formats = vec![
            create_test_format(640, 480, "YUYV", false),
            create_test_format(1920, 1080, "MJPG", true),
            create_test_format(3840, 2160, "H264", false),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        // Photo mode: always select maximum resolution
        assert_eq!(selected.pixel_format, "H264");
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.height, 2160);
    }

    #[test]
    fn test_select_format_picks_absolute_max_resolution() {
        // Photo mode: highest resolution wins regardless of codec type
        let formats = vec![
            create_test_format(640, 480, "YUYV", false),
            create_test_format(1920, 1080, "NV12", false),
            create_test_format(1280, 720, "UYVY", false),
            create_test_format(3840, 2160, "MJPG", true),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        // Photo mode: always select maximum resolution (4K MJPG)
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.height, 2160);
        assert_eq!(selected.pixel_format, "MJPG");
    }

    #[test]
    fn test_select_format_falls_back_to_encoded_when_no_raw() {
        // When no raw formats available, should fall back to encoded formats
        let formats = vec![
            create_test_format(1920, 1080, "MJPG", true),
            create_test_format(3840, 2160, "H264", false),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        // Should select highest resolution encoded format
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.height, 2160);
    }
}
