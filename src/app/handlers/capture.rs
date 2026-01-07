// SPDX-License-Identifier: GPL-3.0-only

//! Capture operations handlers
//!
//! Handles photo capture, video recording, flash, zoom, and timer functionality.

use crate::app::state::{AppModel, CameraMode, Message, RecordingState};
use crate::pipelines::photo::burst_mode::BurstModeConfig;
use cosmic::Task;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Delay in ms before resetting burst mode state after successful capture
const BURST_MODE_SUCCESS_DISPLAY_MS: u64 = 2000;
/// Delay in ms before resetting burst mode state after an error
const BURST_MODE_ERROR_DISPLAY_MS: u64 = 3000;

impl AppModel {
    // =========================================================================
    // Capture Operations Handlers
    // =========================================================================

    /// Create a delayed task that sends a message after the specified milliseconds
    pub(crate) fn delay_task(millis: u64, message: Message) -> Task<cosmic::Action<Message>> {
        Task::perform(
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(millis)).await;
                message
            },
            cosmic::Action::App,
        )
    }

    /// Check if burst mode would be triggered based on current scene brightness
    ///
    /// Returns true if Auto mode would use more than 1 frame (actual burst capture)
    /// or if a fixed frame count > 1 is set.
    pub fn would_use_burst_mode(&self) -> bool {
        use crate::config::BurstModeSetting;

        match self.config.burst_mode_setting {
            BurstModeSetting::Off => false,
            BurstModeSetting::Frames4
            | BurstModeSetting::Frames6
            | BurstModeSetting::Frames8
            | BurstModeSetting::Frames50 => {
                true // Fixed frame counts always use burst
            }
            BurstModeSetting::Auto => {
                // Check scene brightness to determine if burst would be used
                if let Some(frame) = &self.current_frame {
                    use crate::pipelines::photo::burst_mode::burst::{
                        calculate_adaptive_params, estimate_scene_brightness,
                    };
                    let (_luminance, brightness) = estimate_scene_brightness(frame);
                    let params = calculate_adaptive_params(brightness);
                    params.frame_count > 1
                } else {
                    // No frame available, assume burst would be used (conservative)
                    true
                }
            }
        }
    }

    /// Capture the current frame as a photo with the selected filter and zoom
    pub(crate) fn capture_photo(&mut self) -> Task<cosmic::Action<Message>> {
        // Handle scene mode capture separately
        if self.mode == CameraMode::Scene {
            return self.capture_scene();
        }

        // Use HDR+ burst mode if enabled in settings (Auto or fixed frame count)
        // No need to toggle the moon button - HDR+ is automatic when enabled
        if self.config.burst_mode_setting.is_enabled() {
            return self.capture_burst_mode_photo();
        }

        let Some(frame) = &self.current_frame else {
            info!("No frame available to capture");
            return Task::none();
        };

        info!("Capturing photo...");
        self.is_capturing = true;

        // Extract all values from frame before we need to call mutable methods
        let frame_arc = Arc::clone(frame);
        let frame_width = frame.width;
        let frame_height = frame.height;
        let depth_info = frame.depth_data.as_ref().map(|depth_arc| {
            crate::pipelines::photo::encoding::DepthDataInfo {
                values: depth_arc.to_vec(),
                width: frame_width,
                height: frame_height,
            }
        });

        let save_dir = crate::app::get_photo_directory();
        let filter_type = self.selected_filter;
        let zoom_level = self.zoom_level;

        // Calculate crop rectangle based on aspect ratio setting
        let crop_rect = self.photo_aspect_ratio.crop_rect(frame_width, frame_height);
        let crop_rect = if self.photo_aspect_ratio == crate::app::state::PhotoAspectRatio::Native {
            None
        } else {
            Some(crop_rect)
        };

        // Get the encoding format from config
        let encoding_format: crate::pipelines::photo::EncodingFormat =
            self.config.photo_output_format.into();

        // Get camera metadata for DNG encoding (including exposure info and depth data)
        let camera_metadata = self
            .available_cameras
            .get(self.current_camera_index)
            .map(|cam| {
                let mut metadata = crate::pipelines::photo::CameraMetadata {
                    camera_name: Some(cam.name.clone()),
                    camera_driver: cam.device_info.as_ref().map(|info| info.driver.clone()),
                    depth_data: depth_info.clone(),
                    ..Default::default()
                };
                // Read exposure metadata from V4L2 device if available
                if let Some(device_info) = &cam.device_info {
                    let exposure = crate::backends::camera::v4l2_controls::read_exposure_metadata(
                        &device_info.path,
                    );
                    metadata.exposure_time = exposure.exposure_time;
                    metadata.iso = exposure.iso;
                    metadata.gain = exposure.gain;
                }
                metadata
            })
            .unwrap_or_else(|| {
                // Create metadata with just depth data if no camera info available
                crate::pipelines::photo::CameraMetadata {
                    depth_data: depth_info,
                    ..Default::default()
                }
            });

        let save_task = Task::perform(
            async move {
                use crate::pipelines::photo::{
                    EncodingQuality, PhotoPipeline, PostProcessingConfig,
                };
                let mut config = PostProcessingConfig::default();
                config.filter_type = filter_type;
                config.crop_rect = crop_rect;
                config.zoom_level = zoom_level;
                let mut pipeline =
                    PhotoPipeline::with_config(config, encoding_format, EncodingQuality::High);
                pipeline.set_camera_metadata(camera_metadata);
                pipeline
                    .capture_and_save(frame_arc, save_dir)
                    .await
                    .map(|p| p.display().to_string())
            },
            |result| cosmic::Action::App(Message::PhotoSaved(result)),
        );

        let animation_task = Self::delay_task(150, Message::ClearCaptureAnimation);
        Task::batch([save_task, animation_task])
    }

    /// Capture a burst mode photo using multi-frame burst capture
    fn capture_burst_mode_photo(&mut self) -> Task<cosmic::Action<Message>> {
        // Validate state - prevent starting if already active
        if self.burst_mode.is_active() {
            warn!(
                stage = ?self.burst_mode.stage,
                "Cannot start burst mode capture: already active"
            );
            return Task::none();
        }

        // Determine frame count: use config if set, otherwise auto-detect from scene
        let frame_count = match self.config.burst_mode_setting.frame_count() {
            Some(count) => {
                info!(frame_count = count, "Using configured frame count");
                count
            }
            None => {
                // Auto-detect based on scene brightness
                let auto_count = if let Some(frame) = &self.current_frame {
                    use crate::pipelines::photo::burst_mode::burst::{
                        calculate_adaptive_params, estimate_scene_brightness,
                    };
                    let (_luminance, brightness) = estimate_scene_brightness(frame);
                    let params = calculate_adaptive_params(brightness);
                    info!(
                        brightness = ?brightness,
                        auto_frame_count = params.frame_count,
                        "Auto-detected frame count based on scene brightness"
                    );
                    params.frame_count
                } else {
                    8 // Default fallback
                };
                auto_count
            }
        };

        info!(
            frame_count,
            "Starting burst mode capture - collecting frames from stream..."
        );
        self.is_capturing = true;
        self.burst_mode.start_capture(frame_count);

        // If flash is enabled, turn it on for the entire burst capture duration
        if self.flash_enabled {
            info!("Flash enabled - keeping flash on during burst capture");
            self.flash_active = true;
        }

        // Frames will be collected in handle_camera_frame
        // When enough frames are collected, BurstModeFramesCollected message is sent
        Task::none()
    }

    /// Handle when all burst mode frames have been collected
    pub(crate) fn handle_burst_mode_frames_collected(&mut self) -> Task<cosmic::Action<Message>> {
        info!(
            frames = self.burst_mode.frames_captured(),
            "Burst mode frames collected, starting processing"
        );

        // Turn off flash now that capture is complete (before processing)
        if self.flash_active {
            info!("Turning off flash - burst capture complete");
            self.flash_active = false;
        }

        // Update state to processing
        self.burst_mode.start_processing();

        // Take the frames from the buffer
        let frames: Vec<Arc<crate::backends::camera::types::CameraFrame>> =
            self.burst_mode.take_frames();

        if frames.len() < 2 {
            error!("Not enough frames collected for burst mode");
            self.burst_mode.error();
            self.is_capturing = false;
            return Task::none();
        }

        let save_dir = crate::app::get_photo_directory();

        // Calculate crop rectangle based on aspect ratio setting (same as regular photo capture)
        let crop_rect = if let Some(frame) = frames.first() {
            let rect = self.photo_aspect_ratio.crop_rect(frame.width, frame.height);
            if self.photo_aspect_ratio == crate::app::state::PhotoAspectRatio::Native {
                None
            } else {
                Some(rect)
            }
        } else {
            None
        };

        // Get encoding format and camera metadata (including exposure info)
        let encoding_format: crate::pipelines::photo::EncodingFormat =
            self.config.photo_output_format.into();
        let camera_metadata = self
            .available_cameras
            .get(self.current_camera_index)
            .map(|cam| {
                let mut metadata = crate::pipelines::photo::CameraMetadata {
                    camera_name: Some(cam.name.clone()),
                    camera_driver: cam.device_info.as_ref().map(|info| info.driver.clone()),
                    ..Default::default()
                };
                // Read exposure metadata from V4L2 device if available
                if let Some(device_info) = &cam.device_info {
                    let exposure = crate::backends::camera::v4l2_controls::read_exposure_metadata(
                        &device_info.path,
                    );
                    metadata.exposure_time = exposure.exposure_time;
                    metadata.iso = exposure.iso;
                    metadata.gain = exposure.gain;
                }
                metadata
            })
            .unwrap_or_default();

        // Create burst mode config with user's settings
        let mut config = BurstModeConfig::default();
        config.crop_rect = crop_rect;
        config.encoding_format = encoding_format;
        config.camera_metadata = camera_metadata;
        config.save_burst_raw_dng = self.config.save_burst_raw;

        // Night mode effect: significantly boost shadows and brightness for low-light
        // This makes dark scenes much brighter and more visible
        config.shadow_boost = 0.7; // Strong shadow recovery (default is 0.2)
        config.local_contrast = 0.3; // Enhanced local contrast (default is 0.15)
        config.robustness = 1.5; // More aggressive denoising for dark scenes

        // Get selected filter to apply after processing
        let selected_filter = self.selected_filter;

        // Start processing task - BurstModeState handles the communication channels
        let (progress_atomic, result_tx) = self.burst_mode.start_processing_task();

        // Spawn processing on a dedicated OS thread - completely separate from UI/tokio
        // This ensures the event loop stays responsive even during blocking GPU operations
        std::thread::spawn(move || {
            // Create a new tokio runtime for this thread
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for burst mode processing");

            let result = rt.block_on(async move {
                process_burst_mode_frames_with_atomic(
                    frames,
                    save_dir,
                    config,
                    progress_atomic,
                    selected_filter,
                )
                .await
            });
            let _ = result_tx.send(result);
        });

        // Start a timer to periodically poll progress and check for completion (every 100ms)
        Self::delay_task(100, Message::PollBurstModeProgress)
    }

    /// Poll burst mode progress and check for completion
    pub(crate) fn handle_poll_burst_mode_progress(&mut self) -> Task<cosmic::Action<Message>> {
        // Only poll if we're in processing stage
        if self.burst_mode.stage != crate::app::state::BurstModeStage::Processing {
            self.burst_mode.clear_processing_state();
            return Task::none();
        }

        // Update progress from atomic
        self.burst_mode.poll_progress();

        // Check if result is ready (non-blocking)
        if let Some(result) = self.burst_mode.try_get_result() {
            return self.handle_burst_mode_complete(result);
        }

        // Schedule next poll
        Self::delay_task(100, Message::PollBurstModeProgress)
    }

    pub(crate) fn handle_capture(&mut self) -> Task<cosmic::Action<Message>> {
        // If timer countdown is active, abort it
        if self.photo_timer_countdown.is_some() {
            return self.handle_abort_photo_timer();
        }

        // In Photo mode with timer set, start countdown
        if self.mode == CameraMode::Photo
            && self.photo_timer_setting != crate::app::state::PhotoTimerSetting::Off
        {
            let seconds = self.photo_timer_setting.seconds();
            info!(seconds, "Starting photo timer countdown");
            self.photo_timer_countdown = Some(seconds);
            self.photo_timer_tick_start = Some(std::time::Instant::now());
            return Self::delay_task(1000, Message::PhotoTimerTick);
        }

        // Normal capture flow (with flash check)
        if self.mode == CameraMode::Photo && self.flash_enabled && !self.flash_active {
            info!("Flash enabled - showing flash before capture");
            self.flash_active = true;
            return Self::delay_task(1000, Message::FlashComplete);
        }
        self.capture_photo()
    }

    pub(crate) fn handle_toggle_flash(&mut self) -> Task<cosmic::Action<Message>> {
        self.flash_enabled = !self.flash_enabled;
        info!(flash_enabled = self.flash_enabled, "Flash toggled");
        Task::none()
    }

    pub(crate) fn handle_toggle_burst_mode(&mut self) -> Task<cosmic::Action<Message>> {
        use crate::config::BurstModeSetting;
        use cosmic::cosmic_config::CosmicConfigEntry;

        // Toggle between Auto and Off
        self.config.burst_mode_setting = match self.config.burst_mode_setting {
            BurstModeSetting::Off => BurstModeSetting::Auto,
            _ => BurstModeSetting::Off,
        };

        info!(setting = ?self.config.burst_mode_setting, "HDR+ toggled");

        // Save to config
        if let Some(handler) = self.config_handler.as_ref() {
            if let Err(err) = self.config.write_entry(handler) {
                error!(?err, "Failed to save HDR+ setting");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_set_burst_mode_frame_count(
        &mut self,
        index: usize,
    ) -> Task<cosmic::Action<Message>> {
        use cosmic::cosmic_config::CosmicConfigEntry;

        // Don't allow changing frame count during active capture
        if self.burst_mode.is_active() {
            warn!("Cannot change frame count during active capture");
            return Task::none();
        }

        use crate::config::BurstModeSetting;
        // Index 0 = Off, 1 = Auto, 2 = 4 frames, 3 = 6 frames, 4 = 8 frames, 5 = 50 frames
        self.config.burst_mode_setting = match index {
            0 => BurstModeSetting::Off,
            1 => BurstModeSetting::Auto,
            2 => BurstModeSetting::Frames4,
            3 => BurstModeSetting::Frames6,
            4 => BurstModeSetting::Frames8,
            5 => BurstModeSetting::Frames50,
            _ => BurstModeSetting::Auto,
        };
        info!(
            setting = ?self.config.burst_mode_setting,
            "HDR+ setting changed"
        );

        if let Some(handler) = self.config_handler.as_ref() {
            if let Err(err) = self.config.write_entry(handler) {
                error!(?err, "Failed to save burst mode frame count setting");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_cycle_photo_aspect_ratio(&mut self) -> Task<cosmic::Action<Message>> {
        // Get frame dimensions to determine if native matches a defined ratio
        let (width, height) = self
            .current_frame
            .as_ref()
            .map(|f| (f.width, f.height))
            .unwrap_or((0, 0));

        self.photo_aspect_ratio = self.photo_aspect_ratio.next_for_frame(width, height);
        info!(aspect_ratio = ?self.photo_aspect_ratio, "Photo aspect ratio changed");
        Task::none()
    }

    pub(crate) fn handle_flash_complete(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Flash complete - capturing photo");
        self.flash_active = false;
        self.capture_photo()
    }

    pub(crate) fn handle_cycle_photo_timer(&mut self) -> Task<cosmic::Action<Message>> {
        self.photo_timer_setting = self.photo_timer_setting.next();
        info!(
            timer = ?self.photo_timer_setting,
            "Photo timer setting changed"
        );
        Task::none()
    }

    pub(crate) fn handle_photo_timer_tick(&mut self) -> Task<cosmic::Action<Message>> {
        if let Some(remaining) = self.photo_timer_countdown {
            if remaining <= 1 {
                // Countdown complete - capture the photo
                info!("Photo timer countdown complete - capturing");
                self.photo_timer_countdown = None;
                self.photo_timer_tick_start = None;
                // Check if flash is enabled
                if self.flash_enabled && !self.flash_active {
                    info!("Flash enabled - showing flash before capture");
                    self.flash_active = true;
                    return Self::delay_task(1000, Message::FlashComplete);
                }
                return self.capture_photo();
            } else {
                // Continue countdown
                self.photo_timer_countdown = Some(remaining - 1);
                self.photo_timer_tick_start = Some(std::time::Instant::now());
                info!(remaining = remaining - 1, "Photo timer tick");
                return Self::delay_task(1000, Message::PhotoTimerTick);
            }
        }
        Task::none()
    }

    pub(crate) fn handle_abort_photo_timer(&mut self) -> Task<cosmic::Action<Message>> {
        if self.photo_timer_countdown.is_some() {
            info!("Photo timer countdown aborted");
            self.photo_timer_countdown = None;
            self.photo_timer_tick_start = None;
        }
        Task::none()
    }

    pub(crate) fn handle_zoom_in(&mut self) -> Task<cosmic::Action<Message>> {
        // Try hardware zoom first if available
        if self.available_exposure_controls.zoom_absolute.available {
            let range = &self.available_exposure_controls.zoom_absolute;
            if let Some(current) = self.get_v4l2_zoom_value() {
                // Step by ~10% of range or at least 1
                let step = ((range.max - range.min) / 10).max(1);
                let new_zoom = (current + step).min(range.max);
                if new_zoom != current {
                    self.set_v4l2_zoom(new_zoom);
                    // Update zoom_level to reflect hardware zoom for display
                    // Map hardware range to 1.0-10.0 display range
                    let normalized = (new_zoom - range.min) as f32 / (range.max - range.min) as f32;
                    self.zoom_level = 1.0 + normalized * 9.0;
                    debug!(
                        hardware_zoom = new_zoom,
                        display_zoom = self.zoom_level,
                        "Hardware zoom in"
                    );
                }
            }
        } else {
            // Fallback to shader zoom
            let new_zoom = (self.zoom_level + 0.1).min(10.0);
            if (new_zoom - self.zoom_level).abs() > 0.001 {
                self.zoom_level = new_zoom;
                debug!(zoom = self.zoom_level, "Shader zoom in");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_zoom_out(&mut self) -> Task<cosmic::Action<Message>> {
        // Try hardware zoom first if available
        if self.available_exposure_controls.zoom_absolute.available {
            let range = &self.available_exposure_controls.zoom_absolute;
            if let Some(current) = self.get_v4l2_zoom_value() {
                // Step by ~10% of range or at least 1
                let step = ((range.max - range.min) / 10).max(1);
                let new_zoom = (current - step).max(range.min);
                if new_zoom != current {
                    self.set_v4l2_zoom(new_zoom);
                    // Update zoom_level to reflect hardware zoom for display
                    let normalized = (new_zoom - range.min) as f32 / (range.max - range.min) as f32;
                    self.zoom_level = 1.0 + normalized * 9.0;
                    debug!(
                        hardware_zoom = new_zoom,
                        display_zoom = self.zoom_level,
                        "Hardware zoom out"
                    );
                }
            }
        } else {
            // Fallback to shader zoom
            let new_zoom = (self.zoom_level - 0.1).max(1.0);
            if (new_zoom - self.zoom_level).abs() > 0.001 {
                self.zoom_level = new_zoom;
                debug!(zoom = self.zoom_level, "Shader zoom out");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_reset_zoom(&mut self) -> Task<cosmic::Action<Message>> {
        // Reset hardware zoom if available
        if self.available_exposure_controls.zoom_absolute.available {
            let default = self.available_exposure_controls.zoom_absolute.default;
            self.set_v4l2_zoom(default);
            debug!(hardware_zoom = default, "Hardware zoom reset");
        }
        // Always reset shader zoom
        if (self.zoom_level - 1.0).abs() > 0.001 {
            self.zoom_level = 1.0;
            debug!("Shader zoom reset to 1.0");
        }
        Task::none()
    }

    pub(crate) fn handle_photo_saved(
        &mut self,
        result: Result<String, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(path) => {
                info!(path = %path, "Photo saved successfully");
                return Task::done(cosmic::Action::App(Message::RefreshGalleryThumbnail));
            }
            Err(err) => {
                error!(error = %err, "Failed to save photo");
            }
        }
        Task::none()
    }

    /// Capture a scene (depth + color + preview + point cloud + mesh)
    pub(crate) fn capture_scene(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(frame) = &self.current_frame else {
            info!("No frame available for scene capture");
            return Task::none();
        };

        let Some((dw, dh, dd)) = &self.preview_3d.latest_depth_data else {
            info!("No depth data available for scene capture");
            return Task::none();
        };

        // Extract all data first before calling mutable methods
        let rgb_data = frame.data.to_vec();
        let rgb_width = frame.width;
        let rgb_height = frame.height;
        let has_depth_data = frame.depth_data.is_some();
        let depth_data = dd.to_vec();
        let depth_width = *dw;
        let depth_height = *dh;

        // Extract point cloud preview if available
        let (preview_data, preview_width, preview_height) =
            if let Some((pw, ph, pdata)) = &self.preview_3d.rendered_preview {
                (Some(pdata.as_ref().clone()), *pw, *ph)
            } else {
                (None, 0, 0)
            };

        info!("Capturing scene...");
        self.is_capturing = true;

        let save_dir = crate::app::get_photo_directory();

        // Get the encoding format from config
        let image_format: crate::pipelines::photo::EncodingFormat =
            self.config.photo_output_format.into();

        // Determine depth format based on camera type
        let depth_format = if self.kinect.is_device && has_depth_data {
            // Kinect native depth is in millimeters
            crate::shaders::DepthFormat::Millimeters
        } else {
            // V4L2 Y10B depth is disparity
            crate::shaders::DepthFormat::Disparity16
        };
        let mirror = self.config.mirror_preview;

        let save_task = Task::perform(
            async move {
                use crate::pipelines::scene::{
                    CameraIntrinsics, SceneCaptureConfig, capture_scene,
                };

                // Get registration data from the shader processor (same data used for preview)
                let registration_data =
                    match crate::shaders::get_point_cloud_registration_data().await {
                        Ok(Some(shader_reg)) => {
                            // Calculate registration scale factors for high-res RGB
                            // Same logic as in point_cloud/processor.rs and mesh/processor.rs
                            let reg_scale_x = rgb_width as f32 / 640.0;
                            let reg_scale_y = reg_scale_x; // Same as X to maintain aspect ratio
                            let reg_y_offset = 0i32; // Top-aligned crop

                            // Convert from shader RegistrationData to scene RegistrationData
                            Some(crate::pipelines::scene::RegistrationData {
                                registration_table: shader_reg.registration_table,
                                depth_to_rgb_shift: shader_reg.depth_to_rgb_shift,
                                target_offset: shader_reg.target_offset,
                                reg_scale_x,
                                reg_scale_y,
                                reg_y_offset,
                            })
                        }
                        Ok(None) => {
                            tracing::warn!("No registration data available from shader processor");
                            None
                        }
                        Err(e) => {
                            tracing::warn!("Failed to get registration data: {}", e);
                            None
                        }
                    };

                let config = SceneCaptureConfig {
                    image_format,
                    intrinsics: CameraIntrinsics::default(),
                    depth_format,
                    mirror,
                    registration: registration_data,
                };

                capture_scene(
                    &rgb_data,
                    rgb_width,
                    rgb_height,
                    &depth_data,
                    depth_width,
                    depth_height,
                    preview_data.as_deref(),
                    preview_width,
                    preview_height,
                    save_dir,
                    config,
                )
                .await
                .map(|result| result.scene_dir.display().to_string())
            },
            |result| cosmic::Action::App(Message::SceneSaved(result)),
        );

        let animation_task = Self::delay_task(150, Message::ClearCaptureAnimation);
        Task::batch([save_task, animation_task])
    }

    pub(crate) fn handle_scene_saved(
        &mut self,
        result: Result<String, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(path) => {
                info!(path = %path, "Scene saved successfully");
                return Task::done(cosmic::Action::App(Message::RefreshGalleryThumbnail));
            }
            Err(err) => {
                error!(error = %err, "Failed to save scene");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_clear_capture_animation(&mut self) -> Task<cosmic::Action<Message>> {
        self.is_capturing = false;
        Task::none()
    }

    pub(crate) fn handle_toggle_recording(&mut self) -> Task<cosmic::Action<Message>> {
        if self.recording.is_recording() {
            if let Some(sender) = self.recording.take_stop_sender() {
                info!("Sending stop signal to recorder");
                let _ = sender.send(());
            }
            self.recording = RecordingState::Idle;
        } else {
            if self
                .available_cameras
                .get(self.current_camera_index)
                .is_none()
            {
                error!("No camera available for recording");
                return Task::none();
            }
            if self.active_format.is_none() {
                error!("No active format for recording");
                return Task::none();
            }
            return Task::done(cosmic::Action::App(Message::StartRecordingAfterDelay));
        }
        Task::none()
    }

    pub(crate) fn handle_recording_started(
        &mut self,
        path: String,
    ) -> Task<cosmic::Action<Message>> {
        info!(path = %path, "Recording started successfully");
        Self::delay_task(1000, Message::UpdateRecordingDuration)
    }

    pub(crate) fn handle_recording_stopped(
        &mut self,
        result: Result<String, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.recording = RecordingState::Idle;

        match result {
            Ok(path) => {
                info!(path = %path, "Recording saved successfully");
                return Task::done(cosmic::Action::App(Message::RefreshGalleryThumbnail));
            }
            Err(err) => {
                error!(error = %err, "Failed to save recording");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_update_recording_duration(&mut self) -> Task<cosmic::Action<Message>> {
        if self.recording.is_recording() {
            return Self::delay_task(1000, Message::UpdateRecordingDuration);
        }
        Task::none()
    }

    pub(crate) fn handle_start_recording_after_delay(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(camera) = self.available_cameras.get(self.current_camera_index) else {
            error!("Camera disappeared");
            self.recording = RecordingState::Idle;
            return Task::none();
        };

        let Some(format) = &self.active_format else {
            error!("Format disappeared");
            self.recording = RecordingState::Idle;
            return Task::none();
        };

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("VID_{}.mp4", timestamp);
        let save_dir = crate::app::get_photo_directory();
        let output_path = save_dir.join(&filename);

        info!(
            device = %camera.path,
            width = format.width,
            height = format.height,
            fps = ?format.framerate,
            output = %output_path.display(),
            "Starting video recording"
        );

        let device_path = camera.path.clone();
        let metadata_path = camera.metadata_path.clone();
        let width = format.width;
        let height = format.height;
        let framerate = format.framerate.unwrap_or(30);
        let pixel_format = format.pixel_format.clone();

        let audio_device = self
            .available_audio_devices
            .get(self.current_audio_device_index)
            .map(|dev| format!("pipewire-serial-{}", dev.serial));

        let selected_encoder = self
            .available_video_encoders
            .get(self.current_video_encoder_index)
            .cloned();

        let bitrate_kbps = self.config.bitrate_preset.bitrate_kbps(width, height);

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let path_for_message = output_path.display().to_string();
        self.recording = RecordingState::start(path_for_message.clone(), stop_tx);

        let recording_task = Task::perform(
            async move {
                use crate::pipelines::video::{
                    AudioChannels, AudioQuality, EncoderConfig, VideoQuality, VideoRecorder,
                };

                let config = EncoderConfig {
                    video_quality: VideoQuality::High,
                    audio_quality: AudioQuality::High,
                    audio_channels: AudioChannels::Stereo,
                    width,
                    height,
                    bitrate_override_kbps: Some(bitrate_kbps),
                };

                let recorder = match VideoRecorder::new(
                    &device_path,
                    metadata_path.as_deref(),
                    width,
                    height,
                    framerate,
                    &pixel_format,
                    output_path.clone(),
                    config,
                    audio_device.is_some(),
                    audio_device.as_deref(),
                    None,
                    selected_encoder.as_ref(),
                ) {
                    Ok(r) => r,
                    Err(e) => return Err(e),
                };

                if let Err(e) = recorder.start() {
                    return Err(e);
                }

                let path = output_path.display().to_string();
                let _ = stop_rx.await;

                tokio::task::spawn_blocking(move || {
                    recorder.stop().map(|_| path).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))
            },
            |result| cosmic::Action::App(Message::RecordingStopped(result)),
        );

        let start_signal = Task::done(cosmic::Action::App(Message::RecordingStarted(
            path_for_message,
        )));

        Task::batch([start_signal, recording_task])
    }

    /// Handle burst mode progress update
    pub(crate) fn handle_burst_mode_progress(
        &mut self,
        progress: f32,
    ) -> Task<cosmic::Action<Message>> {
        self.burst_mode.processing_progress = progress;

        debug!(
            progress,
            stage = ?self.burst_mode.stage,
            "Burst mode progress"
        );
        Task::none()
    }

    /// Handle burst mode capture complete
    pub(crate) fn handle_burst_mode_complete(
        &mut self,
        result: Result<String, String>,
    ) -> Task<cosmic::Action<Message>> {
        self.is_capturing = false;

        match result {
            Ok(path) => {
                info!(path, "Burst mode capture complete");
                self.burst_mode.complete();

                // Reset burst mode state after a short delay
                let reset_task =
                    Self::delay_task(BURST_MODE_SUCCESS_DISPLAY_MS, Message::ResetBurstModeState);

                // Trigger the same photo saved flow
                let saved_task = Task::done(cosmic::Action::App(Message::PhotoSaved(Ok(path))));

                Task::batch([saved_task, reset_task])
            }
            Err(e) => {
                error!(error = %e, "Burst mode capture failed");
                self.burst_mode.error();

                // Reset after showing error
                Self::delay_task(BURST_MODE_ERROR_DISPLAY_MS, Message::ResetBurstModeState)
            }
        }
    }
}

/// Async function to process collected burst mode frames (GPU-only)
///
/// Uses the unified GPU pipeline for all processing:
/// 1. Initialize GPU pipeline
/// 2. Select reference frame (GPU sharpness)
/// 3. Align frames (GPU)
/// 4. Merge frames (GPU spatial or FFT)
/// 5. Apply tone mapping (GPU)
/// 6. Apply selected filter (GPU)
/// 7. Apply aspect ratio crop (if configured)
/// 8. Save output
///
/// Progress updates are sent via the provided atomic counter (progress * 1000).
async fn process_burst_mode_frames_with_atomic(
    frames: Vec<Arc<crate::backends::camera::types::CameraFrame>>,
    save_dir: PathBuf,
    config: BurstModeConfig,
    progress_atomic: Arc<std::sync::atomic::AtomicU32>,
    filter: crate::app::FilterType,
) -> Result<String, String> {
    use crate::pipelines::photo::burst_mode::{
        ProgressCallback, export_burst_frames_dng, process_burst_mode, save_output,
    };

    info!(
        frame_count = frames.len(),
        crop_rect = ?config.crop_rect,
        encoding_format = ?config.encoding_format,
        save_burst_raw_dng = config.save_burst_raw_dng,
        filter = ?filter,
        "Processing burst mode frames (GPU-only FFT pipeline)"
    );

    // Store fields before moving config
    let crop_rect = config.crop_rect;
    let encoding_format = config.encoding_format;
    let camera_metadata = config.camera_metadata.clone();
    let save_burst_raw_dng = config.save_burst_raw_dng;

    // Export raw burst frames as DNG if enabled (before processing)
    if save_burst_raw_dng {
        match export_burst_frames_dng(&frames, save_dir.clone(), &camera_metadata).await {
            Ok(burst_dir) => {
                info!(burst_dir = %burst_dir.display(), "Raw burst frames saved as DNG");
            }
            Err(e) => {
                error!(error = %e, "Failed to export raw burst frames as DNG");
                // Continue with processing even if export fails
            }
        }
    }

    // Create progress callback that updates the atomic counter
    let progress_callback: ProgressCallback = Arc::new(move |progress: f32| {
        let progress_int = (progress * 1000.0) as u32;
        progress_atomic.store(progress_int, std::sync::atomic::Ordering::Relaxed);
    });

    // Process using the unified GPU pipeline with progress reporting
    let merged = process_burst_mode(frames, config, Some(progress_callback)).await?;

    // Save output with optional crop, filter, and selected encoding format
    let output_path = save_output(
        &merged,
        save_dir,
        crop_rect,
        encoding_format,
        camera_metadata,
        Some(filter),
    )
    .await?;

    info!(path = %output_path.display(), "Burst mode photo saved");
    Ok(output_path.display().to_string())
}
