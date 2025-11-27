// SPDX-License-Identifier: MPL-2.0

//! Camera operation logic (switching cameras, changing formats, etc.)

use crate::app::format_picker::preferences as format_selection;
use crate::app::state::{AppModel, CameraMode};
use crate::backends::camera::types::CameraFormat;
use cosmic::cosmic_config::CosmicConfigEntry;
use tracing::{error, info};

impl AppModel {
    /// Check if switching to a different mode would change the camera format
    /// Returns true if the format would change, false if it would stay the same
    pub fn would_format_change_for_mode(&self, new_mode: CameraMode) -> bool {
        let camera_index = self.current_camera_index;
        if camera_index >= self.available_cameras.len() {
            return true; // No camera available, assume change
        }

        let camera = &self.available_cameras[camera_index];
        let camera_path = &camera.path;

        // Get formats for the new mode
        let backend = crate::backends::camera::get_backend();
        let device = crate::backends::camera::types::CameraDevice {
            name: camera.name.clone(),
            path: camera_path.clone(),
            metadata_path: camera.metadata_path.clone(),
        };
        let formats_for_new_mode = backend.get_formats(&device, new_mode == CameraMode::Video);

        // Helper to check saved settings for a mode
        let check_saved_settings =
            |settings_map: &std::collections::HashMap<String, crate::config::FormatSettings>| {
                settings_map
                    .get(camera_path)
                    .and_then(|settings| {
                        formats_for_new_mode.iter().find(|f| {
                            f.width == settings.width
                                && f.height == settings.height
                                && f.framerate == settings.framerate
                                && f.pixel_format == settings.pixel_format
                        })
                    })
                    .cloned()
            };

        // Determine what format would be selected in the new mode
        // Note: We don't use current format as fallback to avoid cross-contamination
        let would_select_format = match new_mode {
            CameraMode::Photo => {
                // Photo mode: saved settings > max resolution
                check_saved_settings(&self.config.photo_settings).or_else(|| {
                    format_selection::select_max_resolution_format(&formats_for_new_mode)
                })
            }
            CameraMode::Video => {
                // Video mode: saved settings > optimal video defaults
                check_saved_settings(&self.config.video_settings).or_else(|| {
                    format_selection::select_first_time_video_format(&formats_for_new_mode)
                })
            }
        };

        // Compare with current format
        match (self.active_format.as_ref(), would_select_format.as_ref()) {
            (Some(current), Some(would_select)) => {
                // Both formats exist - compare them
                current.width != would_select.width
                    || current.height != would_select.height
                    || current.framerate != would_select.framerate
                    || current.pixel_format != would_select.pixel_format
            }
            (None, Some(_)) | (Some(_), None) => true, // One exists, one doesn't - format changes
            (None, None) => false,                     // Both None - no change
        }
    }

    /// Save current camera and format settings to config
    /// Saves settings for both photo and video modes per camera
    pub fn save_settings(&mut self) {
        let Some(handler) = self.config_handler.as_ref() else {
            error!("Cannot save settings - no config handler");
            return;
        };

        // Get current camera path and format
        let Some(camera) = self.available_cameras.get(self.current_camera_index) else {
            error!(
                index = self.current_camera_index,
                "Cannot save settings - invalid camera index"
            );
            return;
        };

        let Some(format) = self.active_format.as_ref() else {
            error!("Cannot save settings - no active format");
            return;
        };

        // Create FormatSettings for this camera
        let format_settings = crate::config::FormatSettings {
            width: format.width,
            height: format.height,
            framerate: format.framerate,
            pixel_format: format.pixel_format.clone(),
        };

        // Store in per-camera settings based on current mode
        let mode_name = match self.mode {
            CameraMode::Photo => {
                self.config
                    .photo_settings
                    .insert(camera.path.clone(), format_settings);
                "Photo"
            }
            CameraMode::Video => {
                self.config
                    .video_settings
                    .insert(camera.path.clone(), format_settings);
                "Video"
            }
        };

        // Save to disk
        if let Err(err) = self.config.write_entry(handler) {
            error!(?err, "Failed to save {} settings", mode_name);
        } else {
            info!(
                mode = mode_name,
                camera_path = %camera.path,
                width = format.width,
                height = format.height,
                framerate = ?format.framerate,
                pixel_format = %format.pixel_format,
                "{} settings saved to config", mode_name
            );
        }
    }

    /// Restore video format from saved settings for a camera
    /// Returns the saved format if available, None otherwise
    fn restore_video_format_from_settings(&self, camera_path: &str) -> Option<CameraFormat> {
        if let Some(settings) = self.config.video_settings.get(camera_path) {
            info!(camera_path = %camera_path, "Video mode: attempting to restore saved settings");

            // Try to find exact match for saved settings
            self.available_formats
                .iter()
                .find(|f| {
                    f.width == settings.width
                        && f.height == settings.height
                        && f.framerate == settings.framerate
                        && f.pixel_format == settings.pixel_format
                })
                .cloned()
        } else {
            None
        }
    }

    /// Restore photo format from saved settings for a camera
    /// Returns the saved format if available, None otherwise
    fn restore_photo_format_from_settings(&self, camera_path: &str) -> Option<CameraFormat> {
        if let Some(settings) = self.config.photo_settings.get(camera_path) {
            info!(camera_path = %camera_path, "Photo mode: attempting to restore saved settings");

            // Try to find exact match for saved settings
            self.available_formats
                .iter()
                .find(|f| {
                    f.width == settings.width
                        && f.height == settings.height
                        && f.framerate == settings.framerate
                        && f.pixel_format == settings.pixel_format
                })
                .cloned()
        } else {
            None
        }
    }

    /// Select format for video mode, using saved settings or first-time defaults
    fn select_video_format(&self, camera_path: &str) -> Option<CameraFormat> {
        // Priority: saved settings > optimal video defaults
        // Note: We don't use find_current_format_if_valid() here to avoid
        // cross-contamination between photo and video mode settings
        self.restore_video_format_from_settings(camera_path)
            .or_else(|| {
                info!("First-time video mode: selecting highest resolution with >= 25 fps, prefer up to 60 fps");
                format_selection::select_first_time_video_format(&self.available_formats)
            })
    }

    /// Select format for photo mode, using saved settings or max resolution
    fn select_photo_format(&self, camera_path: &str) -> Option<CameraFormat> {
        // Priority: saved settings > optimal photo defaults (max resolution)
        // Note: We don't use find_current_format_if_valid() here to avoid
        // cross-contamination between photo and video mode settings
        self.restore_photo_format_from_settings(camera_path)
            .or_else(|| {
                info!("First-time photo mode: selecting maximum resolution");
                format_selection::select_max_resolution_format(&self.available_formats)
            })
    }

    /// Switch to a different camera or update format after camera/mode change
    /// This consolidates the logic shared by SwitchCamera, SetMode, and SelectCamera messages
    pub fn switch_camera_or_mode(&mut self, camera_index: usize, mode: CameraMode) {
        if camera_index >= self.available_cameras.len() {
            return;
        }

        let camera = &self.available_cameras[camera_index];
        let _device_path = if !camera.path.is_empty() {
            &camera.path
        } else {
            "/dev/video0"
        };
        let _metadata_path = if !camera.path.is_empty() {
            camera.metadata_path.as_deref()
        } else {
            None
        };
        let camera_path = camera.path.clone();

        // Get formats for this camera using PipeWire backend
        let backend = crate::backends::camera::get_backend();
        let device = crate::backends::camera::types::CameraDevice {
            name: camera.name.clone(),
            path: camera_path.clone(),
            metadata_path: camera.metadata_path.clone(),
        };
        self.available_formats = backend.get_formats(&device, mode == CameraMode::Video);

        // Format selection logic: both modes use saved settings, current format, or defaults
        self.active_format = match mode {
            CameraMode::Photo => self.select_photo_format(&camera_path),
            CameraMode::Video => self.select_video_format(&camera_path),
        };

        // Update all dropdown options
        self.update_all_dropdowns();

        // Save last used camera and settings
        self.config.last_camera_path = Some(camera_path);
        self.save_settings();
    }

    /// Change to a specific format (used by consolidated mode dropdown)
    pub fn change_format(&mut self, format: crate::backends::camera::types::CameraFormat) {
        info!(format = %format, "Switched to format");
        // Set new format - subscription will detect change and call manager.recreate()
        // No need to manually clear pipeline - manager handles stop→create atomically
        self.active_format = Some(format);
        self.current_frame = None;
        self.update_all_dropdowns();
        self.save_settings();
    }

    /// Change pixel format while preserving resolution and framerate
    pub fn change_pixel_format(&mut self, pixel_format: String) {
        if let Some(current) = &self.active_format {
            let width = current.width;
            let height = current.height;
            let framerate = current.framerate;

            if let Some(fmt) =
                format_selection::find_format_with_criteria(&self.available_formats, |f| {
                    f.width == width
                        && f.height == height
                        && f.framerate == framerate
                        && f.pixel_format == pixel_format
                })
            {
                info!(format = %fmt, "Switched to format");
                // Set new format - subscription will detect change and call manager.recreate()
                // No need to manually clear pipeline - manager handles stop→create atomically
                self.active_format = Some(fmt);
                self.current_frame = None;
                self.update_codec_options();
                self.save_settings();
            }
        }
    }

    /// Change resolution while trying to preserve pixel format and framerate
    pub fn change_resolution(&mut self, width: u32, height: u32) {
        let current_pixel_format = self.active_format.as_ref().map(|f| f.pixel_format.clone());
        let current_framerate = self.active_format.as_ref().and_then(|f| f.framerate);

        // Try to preserve both pixel format and framerate
        let new_format =
            format_selection::find_format_with_criteria(&self.available_formats, |f| {
                f.width == width
                    && f.height == height
                    && current_pixel_format
                        .as_ref()
                        .map_or(true, |pf| &f.pixel_format == pf)
                    && current_framerate.map_or(true, |fps| f.framerate == Some(fps))
            })
            .or_else(|| {
                // Fall back to best format for this resolution
                let formats_for_res: Vec<_> = self
                    .available_formats
                    .iter()
                    .filter(|f| f.width == width && f.height == height)
                    .cloned()
                    .collect();
                format_selection::select_best_codec(&formats_for_res)
            });

        if let Some(fmt) = new_format {
            info!(format = %fmt, "Switched to format");
            // Set new format - subscription will detect change and call manager.recreate()
            // Manager handles stop→create atomically, preventing race conditions
            self.active_format = Some(fmt);
            self.current_frame = None;
            self.update_framerate_options();
            self.update_pixel_format_options();
            self.update_codec_options();
            self.save_settings();
        }
    }

    /// Change framerate while trying to preserve resolution and pixel format
    pub fn change_framerate(&mut self, fps: u32) {
        if let Some(current) = &self.active_format {
            let width = current.width;
            let height = current.height;
            let pixel_format = current.pixel_format.clone();

            let new_format =
                format_selection::find_format_with_criteria(&self.available_formats, |f| {
                    f.width == width
                        && f.height == height
                        && f.pixel_format == pixel_format
                        && f.framerate == Some(fps)
                })
                .or_else(|| {
                    // Fall back to best format for this resolution and framerate
                    let formats_for_fps: Vec<_> = self
                        .available_formats
                        .iter()
                        .filter(|f| {
                            f.width == width && f.height == height && f.framerate == Some(fps)
                        })
                        .cloned()
                        .collect();
                    format_selection::select_best_codec(&formats_for_fps)
                });

            if let Some(fmt) = new_format {
                info!(format = %fmt, "Switched to format");
                // Set new format - subscription will detect change and call manager.recreate()
                // Manager handles stop→create atomically, preventing race conditions
                self.active_format = Some(fmt);
                self.current_frame = None;
                self.update_pixel_format_options();
                self.update_codec_options();
                self.save_settings();
            }
        }
    }
}
