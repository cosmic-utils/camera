// SPDX-License-Identifier: GPL-3.0-only

//! Camera operation logic (switching cameras, changing formats, etc.)

use crate::app::format_picker::preferences as format_selection;
use crate::app::state::{AppModel, CameraMode};
use crate::backends::camera::types::{CameraFormat, Framerate};
use crate::config::BurstModeSetting;
use cosmic::cosmic_config::CosmicConfigEntry;
use tracing::{error, info};

/// Helper to compare Framerate with config's u32 framerate
/// Config stores integer fps, so we compare using the integer value
fn framerate_matches_config(framerate: Option<&Framerate>, config_fps: Option<u32>) -> bool {
    match (framerate, config_fps) {
        (Some(fr), Some(fps)) => fr.matches_int(fps),
        (None, None) => true,
        _ => false,
    }
}

impl AppModel {
    /// Whether the format picker should be hidden for the current mode.
    /// Libcamera handles resolution automatically in Photo / Video /
    /// Timelapse modes; View doesn't expose any resolution controls
    /// either (it's a passive viewer with no top-bar buttons).
    pub fn is_format_picker_hidden(&self) -> bool {
        matches!(
            self.mode,
            CameraMode::Photo | CameraMode::Video | CameraMode::Timelapse | CameraMode::View
        )
    }

    /// Whether the HDR+/burst mode button should be visible.
    ///
    /// - Off setting: hidden
    /// - Auto with 1 frame (bright scene) AND not overridden: hidden
    /// - Auto with >1 frames OR overridden: visible
    /// - Fixed frame counts: always visible
    pub fn should_show_burst_button(&self) -> bool {
        match self.config.burst_mode_setting {
            BurstModeSetting::Off => false,
            BurstModeSetting::Auto => {
                self.auto_detected_frame_count > 1 || self.hdr_override_disabled
            }
            _ => true,
        }
    }

    /// Ordered list of available camera modes (left → right in the carousel).
    pub fn available_modes(&self) -> Vec<CameraMode> {
        let mut modes = vec![
            CameraMode::Timelapse,
            CameraMode::Video,
            CameraMode::Photo,
            CameraMode::View,
        ];
        if self.config.virtual_camera_enabled {
            modes.push(CameraMode::Virtual);
        }
        modes
    }

    /// Start a blur transition, capturing the current frame rotation for use during blur.
    /// This ensures the blurred frame uses the rotation of the camera that produced it,
    /// not the rotation of the camera being switched to.
    pub fn start_blur_transition(&mut self) {
        self.blur_frame_rotation = self.current_frame_rotation;
        self.blur_frame_mirror = self.should_mirror_preview();
        let _ = self.transition_state.start();
    }

    /// Start a blur transition with custom duration
    pub fn start_blur_transition_with_duration(&mut self, duration_ms: u64, disable_ui: bool) {
        self.blur_frame_rotation = self.current_frame_rotation;
        self.blur_frame_mirror = self.should_mirror_preview();
        let _ = self
            .transition_state
            .start_with_duration(duration_ms, disable_ui);
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
        // Convert Framerate to integer for config storage
        let format_settings = crate::config::FormatSettings {
            width: format.width,
            height: format.height,
            framerate: format.framerate.map(|fr| fr.as_int()),
            pixel_format: format.pixel_format.clone(),
        };

        // Store in per-camera settings based on current mode.
        // Virtual / Timelapse / View share Photo's per-camera format slot
        // (View is a passive viewer with no format choice of its own).
        let mode_name = match self.mode {
            CameraMode::Photo | CameraMode::Virtual | CameraMode::Timelapse | CameraMode::View => {
                self.config
                    .photo_settings
                    .insert(camera.path.clone(), format_settings);
                match self.mode {
                    CameraMode::Photo => "Photo",
                    CameraMode::Virtual => "Virtual",
                    CameraMode::Timelapse => "Timelapse",
                    CameraMode::View => "View",
                    _ => unreachable!(),
                }
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

    /// Restore format from saved settings for a camera, given a settings map.
    /// Returns the saved format if available, None otherwise.
    fn restore_format_from_settings(
        &self,
        camera_path: &str,
        mode_label: &str,
        settings_map: &std::collections::HashMap<String, crate::config::FormatSettings>,
    ) -> Option<CameraFormat> {
        if let Some(settings) = settings_map.get(camera_path) {
            info!(camera_path = %camera_path, "{} mode: attempting to restore saved settings", mode_label);

            // Try to find exact match for saved settings
            self.available_formats
                .iter()
                .find(|f| {
                    f.width == settings.width
                        && f.height == settings.height
                        && framerate_matches_config(f.framerate.as_ref(), settings.framerate)
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
        self.restore_format_from_settings(camera_path, "Video", &self.config.video_settings)
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
        self.restore_format_from_settings(camera_path, "Photo", &self.config.photo_settings)
            .or_else(|| {
                info!("First-time photo mode: selecting maximum resolution");
                format_selection::select_max_resolution_format(&self.available_formats)
            })
    }

    /// Switch to a different camera or update format after camera/mode change
    /// This consolidates the logic shared by SwitchCamera, SetMode, and SelectCamera messages
    /// Select format from cached available_formats without querying hardware.
    /// Used during pipeline restart to avoid blocking the UI thread.
    /// Select format from cached available_formats and update UI dropdowns.
    /// Does NOT save to disk — call save_settings() separately (or defer it)
    /// to avoid blocking the UI thread on eMMC writes.
    pub fn select_format_from_cache(&mut self, mode: CameraMode) {
        let camera_path = self
            .available_cameras
            .get(self.current_camera_index)
            .map(|c| c.path.clone())
            .unwrap_or_default();

        self.active_format = match mode {
            CameraMode::Photo | CameraMode::Virtual | CameraMode::Timelapse | CameraMode::View => {
                self.select_photo_format(&camera_path)
            }
            CameraMode::Video => self.select_video_format(&camera_path),
        };

        self.update_all_dropdowns();
        self.config.last_camera_path = Some(camera_path);
    }

    pub fn switch_camera_or_mode(&mut self, camera_index: usize, mode: CameraMode) {
        if camera_index >= self.available_cameras.len() {
            return;
        }

        let camera = &self.available_cameras[camera_index];
        let camera_path = camera.path.clone();

        // Get formats for this camera using configured backend.
        // Non-blocking: returns cached formats if the pipeline lock is contended.
        let backend = crate::backends::camera::create_backend();
        self.available_formats = backend.get_formats(camera, mode == CameraMode::Video);

        // Format selection logic: both modes use saved settings, current format, or defaults.
        // Virtual / Timelapse / View use the same format selection as Photo.
        self.active_format = match mode {
            CameraMode::Photo | CameraMode::Virtual | CameraMode::Timelapse | CameraMode::View => {
                self.select_photo_format(&camera_path)
            }
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
                        .is_none_or(|pf| &f.pixel_format == pf)
                    && current_framerate.is_none_or(|fps| f.framerate == Some(fps))
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
        self.change_framerate_optional(Some(fps));
    }

    /// Change framerate while trying to preserve resolution and pixel format
    /// Accepts Option<u32> to support VFR (None = libcamera-managed dynamic framerate)
    pub fn change_framerate_optional(&mut self, fps: Option<u32>) {
        if let Some(current) = &self.active_format {
            let width = current.width;
            let height = current.height;
            let pixel_format = current.pixel_format.clone();

            // Helper to check if framerate matches target
            let framerate_matches = |f: &CameraFormat| -> bool {
                match fps {
                    Some(target_fps) => f
                        .framerate
                        .map(|fr| fr.matches_int(target_fps))
                        .unwrap_or(false),
                    None => f.framerate.is_none(), // VFR - looking for framerate=None
                }
            };

            let new_format =
                format_selection::find_format_with_criteria(&self.available_formats, |f| {
                    f.width == width
                        && f.height == height
                        && f.pixel_format == pixel_format
                        && framerate_matches(f)
                })
                .or_else(|| {
                    // Fall back to best format for this resolution and framerate
                    let formats_for_fps: Vec<_> = self
                        .available_formats
                        .iter()
                        .filter(|f| f.width == width && f.height == height && framerate_matches(f))
                        .cloned()
                        .collect();
                    format_selection::select_best_codec(&formats_for_fps)
                });

            if let Some(fmt) = new_format {
                let fps_str = fps
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "Auto".to_string());
                info!(format = %fmt, fps = %fps_str, "Switched to format");
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
