// SPDX-License-Identifier: GPL-3.0-only

//! Format selection handlers
//!
//! Handles mode switching, resolution selection, framerate selection,
//! codec/pixel format selection, and format picker interactions.

use crate::app::state::{
    AppModel, CameraMode, FileSource, Message, PhotoAspectRatio, RecordingState,
};
use crate::app::utils::{parse_codec, parse_resolution};
use cosmic::Task;
use cosmic::cosmic_config::CosmicConfigEntry;
use std::sync::Arc;
use tracing::{error, info, warn};

impl AppModel {
    // =========================================================================
    // Format Selection Handlers
    // =========================================================================

    /// Get the current camera's sensor rotation
    fn current_camera_rotation(&self) -> crate::backends::camera::types::SensorRotation {
        self.available_cameras
            .get(self.current_camera_index)
            .map(|c| c.rotation)
            .unwrap_or_default()
    }

    pub(crate) fn handle_set_mode(&mut self, mode: CameraMode) -> Task<cosmic::Action<Message>> {
        if self.mode == mode {
            return Task::none();
        }

        // When switching away from Virtual mode with a playing video, pause it first
        if self.mode == CameraMode::Virtual
            && matches!(self.virtual_camera_file_source, Some(FileSource::Video(_)))
            && !self.video_file_paused
        {
            info!("Pausing video preview before mode switch");
            self.stop_video_preview_playback();
            self.video_file_paused = true;
        }

        if self.recording.is_recording() {
            if let Some(sender) = self.recording.take_stop_sender() {
                let _ = sender.send(());
            }
            self.recording = RecordingState::Idle;
        }

        let would_change_format = self.would_format_change_for_mode(mode);

        if would_change_format {
            info!("Mode switch will change format - triggering camera reload with blur");
            let _ = self.transition_state.start();
            self.camera_cancel_flag
                .store(true, std::sync::atomic::Ordering::Release);
            self.camera_cancel_flag =
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        } else {
            info!("Mode switch won't change format - keeping same preview");
        }

        self.mode = mode;
        self.zoom_level = 1.0; // Reset zoom when switching modes
        self.switch_camera_or_mode(self.current_camera_index, mode);

        // When switching to Virtual mode with a file source, restore the file source preview
        if mode == CameraMode::Virtual
            && let Some(ref source) = self.virtual_camera_file_source
        {
            let path = match source {
                FileSource::Image(p) | FileSource::Video(p) => p.clone(),
            };
            let is_video = matches!(source, FileSource::Video(_));
            // For video files, use the stored seek position to restore at the correct frame
            let seek_position = if is_video {
                self.video_preview_seek_position
            } else {
                0.0
            };
            info!(
                is_video,
                seek_position, "Restoring file source preview after mode switch"
            );

            return Task::perform(
                async move {
                    use crate::backends::virtual_camera::{
                        get_video_duration, load_preview_frame, load_video_frame_at_position,
                    };

                    // For video files with a seek position, load frame at that position
                    // Otherwise load the first frame
                    let frame = if is_video && seek_position > 0.0 {
                        match load_video_frame_at_position(&path, seek_position) {
                            Ok(frame) => Some(Arc::new(frame)),
                            Err(e) => {
                                warn!(?e, "Failed to load video frame at position");
                                // Fall back to first frame
                                load_preview_frame(&path).ok().map(Arc::new)
                            }
                        }
                    } else {
                        match load_preview_frame(&path) {
                            Ok(frame) => Some(Arc::new(frame)),
                            Err(e) => {
                                warn!(?e, "Failed to load preview frame");
                                None
                            }
                        }
                    };

                    let duration = if is_video {
                        match get_video_duration(&path) {
                            Ok(dur) => Some(dur),
                            Err(e) => {
                                warn!(?e, "Failed to get video duration");
                                None
                            }
                        }
                    } else {
                        None
                    };

                    (frame, duration)
                },
                |(frame, duration)| {
                    cosmic::Action::App(Message::FileSourcePreviewLoaded(frame, duration))
                },
            );
        }

        // Re-query exposure controls when format changes
        if would_change_format {
            return self.query_exposure_controls_task();
        }

        Task::none()
    }

    pub(crate) fn handle_select_mode(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if let Some(format) = self.mode_list.get(index).cloned() {
            info!(
                width = format.width,
                height = format.height,
                framerate = ?format.framerate,
                pixel_format = %format.pixel_format,
                "Switching to mode from consolidated dropdown"
            );
            self.change_format(format);
            let _ = self.transition_state.start();

            // Re-query exposure controls to reset to defaults for new format
            return self.query_exposure_controls_task();
        }
        Task::none()
    }

    pub(crate) fn handle_select_pixel_format(
        &mut self,
        pixel_format: String,
    ) -> Task<cosmic::Action<Message>> {
        info!(pixel_format = %pixel_format, "Switching to pixel format");
        self.change_pixel_format(pixel_format);
        let _ = self.transition_state.start();

        // Re-query exposure controls to get fresh defaults for new format
        self.query_exposure_controls_task()
    }

    pub(crate) fn handle_select_resolution(
        &mut self,
        resolution_str: String,
    ) -> Task<cosmic::Action<Message>> {
        if let Some((width, height)) = parse_resolution(&resolution_str) {
            info!(width, height, "Switching to resolution");
            self.change_resolution(width, height);
            self.zoom_level = 1.0; // Reset zoom when changing resolution
            let _ = self.transition_state.start();

            // Re-query exposure controls to get fresh defaults for new resolution
            return self.query_exposure_controls_task();
        }
        Task::none()
    }

    pub(crate) fn handle_select_framerate(
        &mut self,
        framerate_str: String,
    ) -> Task<cosmic::Action<Message>> {
        // Handle "Auto" for VFR (variable framerate) - libcamera manages dynamically
        if framerate_str == "Auto" {
            info!("Switching to VFR (Auto framerate - libcamera managed)");
            self.change_framerate_optional(None);
            let _ = self.transition_state.start();
            return self.query_exposure_controls_task();
        }

        if let Ok(fps) = framerate_str.parse::<u32>() {
            info!(fps, "Switching to framerate");
            self.change_framerate_optional(Some(fps));
            let _ = self.transition_state.start();

            // Re-query exposure controls to get fresh defaults for new framerate
            return self.query_exposure_controls_task();
        }
        Task::none()
    }

    pub(crate) fn handle_select_codec(
        &mut self,
        codec_str: String,
    ) -> Task<cosmic::Action<Message>> {
        let pixel_format = parse_codec(&codec_str);
        info!(pixel_format = %pixel_format, "Switching to codec");
        self.change_pixel_format(pixel_format);

        // Re-query exposure controls to get fresh defaults for new codec
        self.query_exposure_controls_task()
    }

    pub(crate) fn handle_picker_select_resolution(
        &mut self,
        width: u32,
    ) -> Task<cosmic::Action<Message>> {
        self.picker_selected_resolution = Some(width);
        let current_fps = self.active_format.as_ref().and_then(|f| f.framerate);

        let matching_formats: Vec<(usize, &crate::backends::camera::types::CameraFormat)> = self
            .available_formats
            .iter()
            .enumerate()
            .filter(|(_, fmt)| fmt.width == width)
            .collect();

        if !matching_formats.is_empty() {
            let format_to_apply = if let Some(target_fps) = current_fps {
                let target_int = target_fps.as_int() as i32;
                matching_formats
                    .iter()
                    .find(|(_, fmt)| fmt.framerate == Some(target_fps))
                    .or_else(|| {
                        matching_formats
                            .iter()
                            .filter(|(_, fmt)| fmt.framerate.is_some())
                            .min_by_key(|(_, fmt)| {
                                let fps = fmt.framerate.unwrap().as_int() as i32;
                                (fps - target_int).abs()
                            })
                    })
                    .or_else(|| matching_formats.first())
            } else {
                matching_formats.first()
            };

            if let Some(&(index, _)) = format_to_apply {
                self.active_format = self.available_formats.get(index).cloned();

                if let Some(fmt) = &self.active_format {
                    info!(width, format = %fmt, "Applied resolution with framerate preservation");
                    // Update aspect ratio default for new dimensions (accounting for rotation)
                    self.photo_aspect_ratio = PhotoAspectRatio::default_for_frame_with_rotation(
                        fmt.width,
                        fmt.height,
                        self.current_camera_rotation(),
                    );
                }
                self.zoom_level = 1.0; // Reset zoom when changing resolution
                self.save_settings();
                let _ = self.transition_state.start();
            }
        }
        Task::none()
    }

    pub(crate) fn handle_picker_select_format(
        &mut self,
        index: usize,
    ) -> Task<cosmic::Action<Message>> {
        if index < self.available_formats.len() {
            self.active_format = self.available_formats.get(index).cloned();
            self.format_picker_visible = false;

            if let Some(fmt) = &self.active_format {
                info!(format = %fmt, "Selected format from picker");
                // Update aspect ratio default for new dimensions (accounting for rotation)
                self.photo_aspect_ratio = PhotoAspectRatio::default_for_frame_with_rotation(
                    fmt.width,
                    fmt.height,
                    self.current_camera_rotation(),
                );
            }
            self.zoom_level = 1.0; // Reset zoom when changing format
            self.save_settings();
            let _ = self.transition_state.start();

            // Re-query exposure controls to reset to defaults for new format
            return self.query_exposure_controls_task();
        }
        Task::none()
    }

    pub(crate) fn handle_select_bitrate_preset(
        &mut self,
        index: usize,
    ) -> Task<cosmic::Action<Message>> {
        if index < crate::constants::BitratePreset::ALL.len() {
            let preset = crate::constants::BitratePreset::ALL[index];
            info!(preset = ?preset, "Selected bitrate preset");
            self.config.bitrate_preset = preset;

            if let Some(handler) = self.config_handler.as_ref()
                && let Err(err) = self.config.write_entry(handler)
            {
                error!(?err, "Failed to save bitrate preset setting");
            }
        }
        Task::none()
    }
}
