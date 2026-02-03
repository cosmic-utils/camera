// SPDX-License-Identifier: GPL-3.0-only

//! Dropdown management and update logic

use crate::app::state::AppModel;
use crate::media::Codec;
use std::collections::HashSet;

/// Helper to sort resolutions by total pixel count (highest to lowest)
fn sort_by_pixels_desc(a: &(u32, u32), b: &(u32, u32)) -> std::cmp::Ordering {
    let pixels_a = a.0 * a.1;
    let pixels_b = b.0 * b.1;
    pixels_b.cmp(&pixels_a)
}

impl AppModel {
    /// Update resolution dropdown options (sorted highest to lowest)
    pub fn update_resolution_options(&mut self) {
        // Get all unique resolutions
        let mut available_resolutions: Vec<(u32, u32)> = self
            .available_formats
            .iter()
            .map(|f| (f.width, f.height))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        // Sort by total pixels (highest to lowest)
        available_resolutions.sort_by(sort_by_pixels_desc);

        self.resolution_dropdown_options = available_resolutions
            .into_iter()
            .map(|(w, h)| format!("{}x{}", w, h))
            .collect();
    }

    /// Update framerate dropdown options based on current resolution (sorted highest to lowest)
    /// Includes "Auto" option for VFR (variable framerate) when available
    pub fn update_framerate_options(&mut self) {
        use crate::backends::camera::types::Framerate;

        if let Some(active) = &self.active_format {
            // Get all formats for current resolution
            let formats_for_res: Vec<_> = self
                .available_formats
                .iter()
                .filter(|f| f.width == active.width && f.height == active.height)
                .collect();

            // Check if VFR (framerate=None) is available
            let has_vfr = formats_for_res.iter().any(|f| f.framerate.is_none());

            // Get all unique fixed framerates
            let mut available_framerates: Vec<Framerate> = formats_for_res
                .iter()
                .filter_map(|f| f.framerate)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            // Sort from highest to lowest by actual fps value
            available_framerates.sort_by(|a, b| {
                b.as_f64()
                    .partial_cmp(&a.as_f64())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // Build the options list
            self.framerate_dropdown_options = available_framerates
                .into_iter()
                .map(|fps| fps.to_string())
                .collect();

            // Add "Auto" option at the end for VFR (libcamera dynamic framerate)
            if has_vfr {
                self.framerate_dropdown_options.push("Auto".to_string());
            }
        } else {
            self.framerate_dropdown_options.clear();
        }
    }

    /// Update pixel format dropdown options based on current resolution and framerate (sorted by preference)
    pub fn update_pixel_format_options(&mut self) {
        if let Some(active) = &self.active_format {
            // Get all unique pixel formats for current resolution and framerate
            let mut available_pixel_formats: Vec<String> = self
                .available_formats
                .iter()
                .filter(|f| {
                    f.width == active.width
                        && f.height == active.height
                        && f.framerate == active.framerate
                })
                .map(|f| f.pixel_format.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            // Sort by preference order
            available_pixel_formats.sort_by_key(|pf| pixel_format_rank(pf));

            self.pixel_format_dropdown_options = available_pixel_formats;
        } else {
            self.pixel_format_dropdown_options.clear();
        }
    }

    /// Update codec dropdown options based on current resolution and framerate (sorted by preference)
    pub fn update_codec_options(&mut self) {
        if let Some(active) = &self.active_format {
            // Get all unique codecs (pixel formats) for current resolution and framerate
            let mut available_codecs: Vec<(String, String)> = self
                .available_formats
                .iter()
                .filter(|f| {
                    f.width == active.width
                        && f.height == active.height
                        && f.framerate == active.framerate
                })
                .map(|f| {
                    let desc = get_codec_short_description(&f.pixel_format);
                    let detail = get_codec_display_detail(&f.pixel_format);
                    (f.pixel_format.clone(), format!("{} ({})", desc, detail))
                })
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            // Sort by preference order
            available_codecs.sort_by(|a, b| pixel_format_rank(&a.0).cmp(&pixel_format_rank(&b.0)));

            self.codec_dropdown_options = available_codecs
                .into_iter()
                .map(|(_, formatted)| formatted)
                .collect();
        } else {
            self.codec_dropdown_options.clear();
        }
    }

    /// Update all dropdown options based on current active format
    pub fn update_all_dropdowns(&mut self) {
        self.update_mode_options();
        self.update_resolution_options();
        self.update_pixel_format_options();
        self.update_framerate_options();
        self.update_codec_options();
    }

    /// Update mode dropdown options (consolidated format selector)
    /// Sorted by: resolution descending, then framerate descending, then format alphabetical
    pub fn update_mode_options(&mut self) {
        use crate::backends::camera::types::CameraFormat;

        // Clone all formats and sort them
        let mut modes: Vec<CameraFormat> = self.available_formats.clone();

        // Sort by: resolution descending, framerate descending, format alphabetical
        modes.sort_by(|a, b| {
            let framerate_a = a.framerate.map(|f| f.as_f64()).unwrap_or(0.0);
            let framerate_b = b.framerate.map(|f| f.as_f64()).unwrap_or(0.0);
            let pixels_a = a.width * a.height;
            let pixels_b = b.width * b.height;

            pixels_b
                .cmp(&pixels_a) // resolution (pixels) descending
                .then(
                    framerate_b
                        .partial_cmp(&framerate_a)
                        .unwrap_or(std::cmp::Ordering::Equal),
                ) // framerate descending
                .then(a.pixel_format.cmp(&b.pixel_format)) // format alphabetical
        });

        // Generate display strings
        self.mode_dropdown_options = modes
            .iter()
            .map(|f| {
                let framerate_str = match f.framerate {
                    Some(fr) => format!("{}fps", fr),
                    None => "Auto".to_string(), // VFR - libcamera manages framerate
                };
                let codec_desc = get_codec_short_description(&f.pixel_format);
                let codec_detail = get_codec_display_detail(&f.pixel_format);
                format!(
                    "{}x{} @ {} - {} ({})",
                    f.width, f.height, framerate_str, codec_desc, codec_detail
                )
            })
            .collect();

        // Store the sorted modes list for lookup
        self.mode_list = modes;
    }
}

/// Helper to rank pixel format by preference order
fn pixel_format_rank(pixel_format: &str) -> u32 {
    Codec::from_fourcc(pixel_format).preference_rank()
}

/// Get short codec description for dropdowns
pub fn get_codec_short_description(pixel_format: &str) -> &'static str {
    Codec::from_fourcc(pixel_format).short_description()
}

/// Get codec display detail for parentheses in dropdowns
pub fn get_codec_display_detail(pixel_format: &str) -> &'static str {
    Codec::from_fourcc(pixel_format).display_detail()
}
