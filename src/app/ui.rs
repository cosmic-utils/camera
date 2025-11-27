// SPDX-License-Identifier: MPL-2.0

//! UI helper functions
//!
//! This module contains shared UI utilities that are used by multiple view modules.

use crate::app::state::AppModel;
use crate::backends::camera::types::CameraFormat;
use crate::constants;
use std::collections::HashMap;

impl AppModel {
    /// Group formats by resolution label and return sorted list with best resolution for each label
    ///
    /// This helper is used by the format picker to organize formats by resolution categories
    /// (SD, HD, 720p, 4K, etc.).
    ///
    /// Returns:
    /// - A sorted list of (label, width) pairs representing unique resolution categories
    /// - A map from width to list of (index, format) pairs for that resolution
    pub(crate) fn group_formats_by_label(
        &self,
    ) -> (
        Vec<(&'static str, u32)>,
        HashMap<u32, Vec<(usize, &CameraFormat)>>,
    ) {
        // Group formats by their label (SD, HD, 4K, etc.) and pick the highest resolution for each
        let mut label_to_best_format: HashMap<
            &'static str,
            (u32, u32, Vec<(usize, &CameraFormat)>),
        > = HashMap::new();

        for (idx, fmt) in self.available_formats.iter().enumerate() {
            if let Some(label) = constants::get_resolution_label(fmt.width) {
                let resolution_score = fmt.width * fmt.height;

                label_to_best_format
                    .entry(label)
                    .and_modify(|(best_width, best_score, formats)| {
                        if resolution_score > *best_score {
                            *best_width = fmt.width;
                            *best_score = resolution_score;
                            formats.clear();
                            formats.push((idx, fmt));
                        } else if resolution_score == *best_score && fmt.width == *best_width {
                            formats.push((idx, fmt));
                        }
                    })
                    .or_insert((fmt.width, resolution_score, vec![(idx, fmt)]));
            }
        }

        // Create sorted list of (label, width)
        let mut unique_resolutions: Vec<(&'static str, u32)> = label_to_best_format
            .iter()
            .map(|(&label, &(width, _, _))| (label, width))
            .collect();
        unique_resolutions.sort_by_key(|(_, width)| *width);

        // Create resolution_groups HashMap keyed by width
        let resolution_groups: HashMap<u32, Vec<(usize, &CameraFormat)>> = label_to_best_format
            .iter()
            .map(|(_label, &(width, _, ref formats))| (width, formats.clone()))
            .collect();

        (unique_resolutions, resolution_groups)
    }
}
