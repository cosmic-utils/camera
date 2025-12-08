// SPDX-License-Identifier: GPL-3.0-only

//! Format selection and preference logic

use crate::backends::camera::types::CameraFormat;
use crate::media::Codec;
use tracing::info;

/// Select format with maximum resolution (for Photo mode)
///
/// Photo mode: ALWAYS select maximum resolution, regardless of codec.
/// The user wants the highest quality photo possible.
///
/// Framerate preference:
/// - Prefer 30-60 fps range (at least 30 fps, capped at 60 fps)
/// - If no formats in that range, take the highest available framerate
///
/// Codec preference when multiple formats have same resolution and framerate:
/// Raw formats: YUYV > UYVY > YUY2 > NV12 > YV12 > I420
/// Encoded formats: H.264 > HW-accelerated MJPEG > HW-accelerated > MJPEG > First
pub fn select_max_resolution_format(formats: &[CameraFormat]) -> Option<CameraFormat> {
    if formats.is_empty() {
        return None;
    }

    info!("Photo mode: selecting maximum resolution with optimal framerate");

    // Find max resolution (by total pixels)
    let max_pixels = formats.iter().map(|f| f.width * f.height).max()?;

    // Get all formats with max resolution
    let max_res_formats: Vec<_> = formats
        .iter()
        .filter(|f| f.width * f.height == max_pixels)
        .cloned()
        .collect();

    // Filter to formats with 30-60 fps range (preferred range for photo mode)
    let preferred_fps_formats: Vec<_> = max_res_formats
        .iter()
        .filter(|f| {
            f.framerate
                .map(|fps| fps >= 30 && fps <= 60)
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    if !preferred_fps_formats.is_empty() {
        // Among 30-60 fps formats, prefer highest fps (closer to 60)
        let best_fps = preferred_fps_formats
            .iter()
            .filter_map(|f| f.framerate)
            .max()
            .unwrap_or(30);

        let best_fps_formats: Vec<_> = preferred_fps_formats
            .iter()
            .filter(|f| f.framerate == Some(best_fps))
            .cloned()
            .collect();

        info!(
            resolution = format!("{}x{}", max_res_formats[0].width, max_res_formats[0].height),
            fps = best_fps,
            "Selected format with preferred framerate"
        );
        return select_best_codec(&best_fps_formats);
    }

    // No formats in 30-60 fps range, fall back to highest available framerate
    let max_fps = max_res_formats
        .iter()
        .filter_map(|f| f.framerate)
        .max()
        .unwrap_or(0);

    if max_fps > 0 {
        let max_fps_formats: Vec<_> = max_res_formats
            .iter()
            .filter(|f| f.framerate == Some(max_fps))
            .cloned()
            .collect();

        info!(
            resolution = format!("{}x{}", max_res_formats[0].width, max_res_formats[0].height),
            fps = max_fps,
            "Selected format with highest available framerate (outside 30-60 range)"
        );
        return select_best_codec(&max_fps_formats);
    }

    // No framerate info available, just apply codec preference
    info!(
        resolution = format!("{}x{}", max_res_formats[0].width, max_res_formats[0].height),
        "Selected format without framerate info"
    );
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
                    .or_default()
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

    fn create_test_format_with_fps(
        width: u32,
        height: u32,
        pixel_format: &str,
        hw_accel: bool,
        fps: u32,
    ) -> CameraFormat {
        CameraFormat {
            width,
            height,
            framerate: Some(fps),
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

    #[test]
    fn test_select_format_prefers_30_60_fps_range() {
        // Should prefer formats in 30-60 fps range at max resolution
        let formats = vec![
            create_test_format_with_fps(3840, 2160, "MJPG", true, 15),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 30),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 120),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.framerate, Some(30));
    }

    #[test]
    fn test_select_format_prefers_highest_fps_in_30_60_range() {
        // Should prefer 60 fps over 30 fps when both are available
        let formats = vec![
            create_test_format_with_fps(3840, 2160, "MJPG", true, 30),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 60),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 45),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.framerate, Some(60));
    }

    #[test]
    fn test_select_format_falls_back_to_highest_fps_when_no_30_60() {
        // When no formats in 30-60 fps range, should fall back to highest fps
        let formats = vec![
            create_test_format_with_fps(3840, 2160, "MJPG", true, 5),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 10),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 15),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.framerate, Some(15));
    }

    #[test]
    fn test_select_format_excludes_fps_above_60() {
        // Should not select fps > 60 when 30-60 range is available
        let formats = vec![
            create_test_format_with_fps(3840, 2160, "MJPG", true, 30),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 120),
            create_test_format_with_fps(3840, 2160, "MJPG", true, 240),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.framerate, Some(30));
    }

    #[test]
    fn test_select_format_applies_codec_preference_at_same_fps() {
        // Should apply codec preference when multiple codecs have same fps
        let formats = vec![
            create_test_format_with_fps(3840, 2160, "MJPG", false, 30),
            create_test_format_with_fps(3840, 2160, "YUYV", false, 30),
            create_test_format_with_fps(3840, 2160, "H264", false, 30),
        ];

        let selected = select_max_resolution_format(&formats).unwrap();
        assert_eq!(selected.width, 3840);
        assert_eq!(selected.framerate, Some(30));
        // Raw format (YUYV) should be preferred
        assert_eq!(selected.pixel_format, "YUYV");
    }
}
