// SPDX-License-Identifier: GPL-3.0-only

//! Capture operations handlers
//!
//! Handles photo capture, video recording, flash, zoom, and timer functionality.

use crate::app::state::{AppModel, CameraMode, Message, RecordingState};
use crate::backends::camera::v4l2_controls::read_exposure_metadata;
use crate::pipelines::photo::burst_mode::BurstModeConfig;
use crate::pipelines::photo::burst_mode::burst::{
    calculate_adaptive_params, estimate_scene_brightness,
};
use cosmic::Task;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Configuration for appsrc recording pipeline (libcamera backend)
struct AppsrcRecordingConfig {
    width: u32,
    height: u32,
    framerate: u32,
    format: crate::backends::camera::types::CameraFormat,
    output_path: PathBuf,
    sensor_rotation: crate::backends::camera::types::SensorRotation,
    audio_device: Option<String>,
    selected_encoder: Option<crate::media::encoders::video::EncoderInfo>,
    bitrate_kbps: u32,
}

/// Delay in ms before resetting burst mode state after successful capture
const BURST_MODE_SUCCESS_DISPLAY_MS: u64 = 2000;
/// Delay in ms before resetting burst mode state after an error
const BURST_MODE_ERROR_DISPLAY_MS: u64 = 3000;

impl AppModel {
    // =========================================================================
    // Flash Hardware Helpers
    // =========================================================================

    /// Check if the current camera is a back-facing camera (based on libcamera location property)
    pub(crate) fn is_back_camera(&self) -> bool {
        self.available_cameras
            .get(self.current_camera_index)
            .and_then(|c| c.camera_location.as_deref())
            == Some("back")
    }

    /// Check if hardware flash should be used (back camera with writable flash LEDs)
    pub(crate) fn use_hardware_flash(&self) -> bool {
        self.is_back_camera() && self.flash_hardware.has_devices()
    }

    /// Turn on hardware flash LEDs (only for back cameras with writable hardware)
    pub(crate) fn turn_on_flash_hardware(&self) {
        if self.use_hardware_flash() {
            info!("Turning on hardware flash LEDs");
            crate::flash::all_on(&self.flash_hardware.devices);
        }
    }

    /// Turn off hardware flash LEDs (safe to call even if no hardware)
    pub(crate) fn turn_off_flash_hardware(&self) {
        if !self.flash_hardware.devices.is_empty() {
            crate::flash::all_off(&self.flash_hardware.devices);
        }
    }

    /// Handle dismissing the flash permission error popup
    pub(crate) fn handle_dismiss_flash_error(&mut self) -> Task<cosmic::Action<Message>> {
        self.flash_error_popup = None;
        Task::none()
    }

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
    /// or if a fixed frame count > 1 is set, AND the user hasn't overridden it.
    pub fn would_use_burst_mode(&self) -> bool {
        use crate::config::BurstModeSetting;

        // User override takes precedence
        if self.hdr_override_disabled {
            return false;
        }

        match self.config.burst_mode_setting {
            BurstModeSetting::Off => false,
            BurstModeSetting::Frames4
            | BurstModeSetting::Frames6
            | BurstModeSetting::Frames8
            | BurstModeSetting::Frames50 => true, // Fixed frame counts always use burst
            BurstModeSetting::Auto => {
                // Use the cached auto-detected frame count (updated every 1 second)
                self.auto_detected_frame_count > 1
            }
        }
    }

    /// Build camera metadata (name, driver, exposure info) for photo encoding.
    fn build_camera_metadata(&self) -> crate::pipelines::photo::CameraMetadata {
        self.available_cameras
            .get(self.current_camera_index)
            .map(|cam| {
                let mut metadata = crate::pipelines::photo::CameraMetadata {
                    camera_name: Some(cam.name.clone()),
                    camera_driver: cam.device_info.as_ref().map(|info| info.driver.clone()),
                    ..Default::default()
                };
                if let Some(device_info) = &cam.device_info {
                    let exposure = read_exposure_metadata(&device_info.path);
                    metadata.exposure_time = exposure.exposure_time;
                    metadata.iso = exposure.iso;
                    metadata.gain = exposure.gain;
                }
                metadata
            })
            .unwrap_or_default()
    }

    /// Capture the current frame as a photo with the selected filter and zoom
    pub(crate) fn capture_photo(&mut self) -> Task<cosmic::Action<Message>> {
        // Use HDR+ burst mode only if it would actually be used (frame_count > 1)
        // This respects auto-detected brightness and user override
        if self.would_use_burst_mode() {
            return self.capture_burst_mode_photo();
        }

        // In multistream mode, capture from the raw stream (full sensor resolution)
        // instead of the preview stream (1080p)
        let is_multistream = self
            .available_cameras
            .get(self.current_camera_index)
            .map(|cam| cam.supports_multistream)
            .unwrap_or(false);

        if is_multistream {
            return self.capture_photo_from_raw_stream();
        }

        let Some(frame) = &self.current_frame else {
            info!("No frame available to capture");
            return Task::none();
        };

        info!("Capturing photo...");
        self.is_capturing = true;

        let frame_arc = Arc::clone(frame);
        let save_dir = crate::app::get_photo_directory(&self.config.save_folder_name);
        let filter_type = self.selected_filter;
        let zoom_level = self.zoom_level;

        let rotation = self.current_camera_rotation();

        // Calculate crop rectangle based on aspect ratio setting (accounting for rotation)
        let crop_rect = self.photo_aspect_ratio.optional_crop_rect_with_rotation(
            frame.width,
            frame.height,
            rotation,
        );

        // Get the encoding format from config
        let encoding_format: crate::pipelines::photo::EncodingFormat =
            self.config.photo_output_format.into();

        let camera_metadata = self.build_camera_metadata();

        let save_task = Task::perform(
            async move {
                use crate::pipelines::photo::{
                    EncodingQuality, PhotoPipeline, PostProcessingConfig,
                };
                let config = PostProcessingConfig {
                    filter_type,
                    crop_rect,
                    zoom_level,
                    rotation,
                    ..Default::default()
                };
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

    /// Capture a photo from the raw stream (multistream mode)
    ///
    /// Requests a full-resolution raw frame from the dedicated raw stream,
    /// which bypasses the ISP and captures at the sensor's native resolution.
    fn capture_photo_from_raw_stream(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Capturing photo from raw stream (multistream mode)...");
        self.is_capturing = true;

        // Request a still capture from the raw stream
        self.still_capture_requested
            .store(true, std::sync::atomic::Ordering::Release);

        let still_frame = Arc::clone(&self.latest_still_frame);
        let save_dir = crate::app::get_photo_directory(&self.config.save_folder_name);
        let filter_type = self.selected_filter;
        let zoom_level = self.zoom_level;

        let rotation = self.current_camera_rotation();

        // For raw stream capture, use native aspect ratio (no crop from preview)
        // The raw frame may have a different aspect ratio than the preview
        let photo_aspect_ratio = self.photo_aspect_ratio;

        let encoding_format: crate::pipelines::photo::EncodingFormat =
            self.config.photo_output_format.into();

        let camera_metadata = self.build_camera_metadata();

        let save_task = Task::perform(
            async move {
                use crate::pipelines::photo::{
                    EncodingQuality, PhotoPipeline, PostProcessingConfig,
                };

                // Wait for the raw frame with a timeout
                let timeout = std::time::Duration::from_secs(2);
                let start = std::time::Instant::now();
                let frame = loop {
                    if let Ok(mut guard) = still_frame.lock()
                        && let Some(frame) = guard.take()
                    {
                        break frame;
                    }
                    if start.elapsed() > timeout {
                        return Err("Timeout waiting for raw frame from still stream".to_string());
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                };

                info!(
                    width = frame.width,
                    height = frame.height,
                    format = ?frame.format,
                    "Raw frame captured from still stream"
                );

                let crop_rect = photo_aspect_ratio.optional_crop_rect_with_rotation(
                    frame.width,
                    frame.height,
                    rotation,
                );

                let config = PostProcessingConfig {
                    filter_type,
                    crop_rect,
                    zoom_level,
                    rotation,
                    ..Default::default()
                };
                let mut pipeline =
                    PhotoPipeline::with_config(config, encoding_format, EncodingQuality::High);
                pipeline.set_camera_metadata(camera_metadata);
                pipeline
                    .capture_and_save(Arc::new(frame), save_dir)
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

        // Determine frame count: use config if set, otherwise use cached auto-detected value
        let frame_count = match self.config.burst_mode_setting.frame_count() {
            Some(count) => {
                info!(frame_count = count, "Using configured frame count");
                count
            }
            None => {
                // Use the cached auto-detected frame count (updated every 1 second)
                let auto_count = self.auto_detected_frame_count;
                info!(
                    auto_frame_count = auto_count,
                    "Using cached auto-detected frame count"
                );
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
            if self.use_hardware_flash() {
                info!("Flash enabled - turning on hardware flash during burst capture");
                self.turn_on_flash_hardware();
            } else {
                info!("Flash enabled - keeping screen flash on during burst capture");
                self.flash_active = true;
            }
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
        self.turn_off_flash_hardware();
        if self.flash_active {
            info!("Turning off screen flash - burst capture complete");
            self.flash_active = false;
        }

        // Stop the PipeWire camera stream during HDR+ processing
        // This frees GPU/CPU resources for burst processing
        // The stream will be restarted in handle_burst_mode_complete
        info!("Stopping camera stream for HDR+ processing");
        self.camera_cancel_flag
            .store(true, std::sync::atomic::Ordering::Release);

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

        let save_dir = crate::app::get_photo_directory(&self.config.save_folder_name);

        // Get encoding format and camera metadata (including exposure info)
        let encoding_format: crate::pipelines::photo::EncodingFormat =
            self.config.photo_output_format.into();

        let camera_metadata = self.build_camera_metadata();

        let rotation = self.current_camera_rotation();

        // Calculate crop rectangle based on aspect ratio setting (accounting for rotation)
        let crop_rect = if let Some(frame) = frames.first() {
            self.photo_aspect_ratio.optional_crop_rect_with_rotation(
                frame.width,
                frame.height,
                rotation,
            )
        } else {
            None
        };

        // Create burst mode config with user's settings
        let mut config = BurstModeConfig::default();
        config.crop_rect = crop_rect;
        config.encoding_format = encoding_format;
        config.camera_metadata = camera_metadata;
        config.save_burst_raw_dng = self.config.save_burst_raw;
        config.rotation = rotation;

        // Calculate adaptive processing parameters based on scene brightness
        if let Some(first_frame) = frames.first() {
            let (_luminance, brightness) = estimate_scene_brightness(first_frame);
            let adaptive_params = calculate_adaptive_params(brightness);
            config.shadow_boost = adaptive_params.shadow_boost;
            config.local_contrast = adaptive_params.local_contrast;
            config.robustness = adaptive_params.robustness;
            debug!(
                ?brightness,
                shadow_boost = adaptive_params.shadow_boost,
                local_contrast = adaptive_params.local_contrast,
                robustness = adaptive_params.robustness,
                "Adaptive burst mode parameters applied"
            );
        }

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

    /// Handle periodic brightness evaluation tick
    ///
    /// Evaluates scene brightness every 1 second and updates auto_detected_frame_count.
    /// Only runs when in Auto mode and not overridden.
    pub(crate) fn handle_brightness_evaluation_tick(&mut self) -> Task<cosmic::Action<Message>> {
        use crate::config::BurstModeSetting;
        use crate::pipelines::photo::burst_mode::burst::{
            calculate_adaptive_params, estimate_scene_brightness,
        };

        // Only evaluate in Auto mode when not overridden
        if !matches!(self.config.burst_mode_setting, BurstModeSetting::Auto) {
            return Task::none();
        }

        if self.hdr_override_disabled {
            return Task::none();
        }

        // Evaluate brightness from current frame
        if let Some(frame) = &self.current_frame {
            let (_luminance, brightness) = estimate_scene_brightness(frame);
            let params = calculate_adaptive_params(brightness);

            // Only update and log if frame count changed
            if params.frame_count != self.auto_detected_frame_count {
                debug!(
                    old_count = self.auto_detected_frame_count,
                    new_count = params.frame_count,
                    brightness = ?brightness,
                    "Auto frame count updated based on scene brightness"
                );
                self.auto_detected_frame_count = params.frame_count;
            }
        }

        Task::none()
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
            if self.use_hardware_flash() {
                info!("Flash enabled - turning on hardware flash before capture");
                self.turn_on_flash_hardware();
            } else {
                info!("Flash enabled - showing screen flash before capture");
                self.flash_active = true;
            }
            return Self::delay_task(1000, Message::FlashComplete);
        }
        self.capture_photo()
    }

    pub(crate) fn handle_toggle_flash(&mut self) -> Task<cosmic::Action<Message>> {
        // If trying to enable flash on a back camera with permission errors, show popup
        if !self.flash_enabled && self.is_back_camera() && self.flash_hardware.has_error() {
            warn!("Flash hardware detected but not writable — showing permission error");
            self.flash_error_popup = self.flash_hardware.permission_error.clone();
            return Task::none();
        }

        self.flash_enabled = !self.flash_enabled;
        info!(
            flash_enabled = self.flash_enabled,
            is_back = self.is_back_camera(),
            has_hardware = self.flash_hardware.has_devices(),
            "Flash toggled"
        );
        Task::none()
    }

    pub(crate) fn handle_toggle_burst_mode(&mut self) -> Task<cosmic::Action<Message>> {
        use crate::config::BurstModeSetting;
        use cosmic::cosmic_config::CosmicConfigEntry;

        // Track if config changed (need to save)
        let mut config_changed = false;

        match self.config.burst_mode_setting {
            BurstModeSetting::Off => {
                // Turning ON: go to Auto mode, clear any override
                self.config.burst_mode_setting = BurstModeSetting::Auto;
                self.hdr_override_disabled = false;
                config_changed = true;
            }
            BurstModeSetting::Auto => {
                if self.hdr_override_disabled {
                    // Already overridden - toggle back to enabled
                    self.hdr_override_disabled = false;
                    info!("HDR+ override cleared - auto mode re-enabled");
                } else if self.auto_detected_frame_count > 1 {
                    // HDR+ would be active - set override to disable it
                    self.hdr_override_disabled = true;
                    info!("HDR+ override enabled - auto mode disabled until next toggle");
                } else {
                    // HDR+ already not active (bright scene, 1 frame) - switch to Off
                    self.config.burst_mode_setting = BurstModeSetting::Off;
                    config_changed = true;
                }
            }
            _ => {
                // Fixed frame count modes - toggle override
                if self.hdr_override_disabled {
                    self.hdr_override_disabled = false;
                    info!("HDR+ override cleared for fixed frame count mode");
                } else {
                    self.hdr_override_disabled = true;
                    info!("HDR+ override enabled for fixed frame count mode");
                }
            }
        }

        info!(
            setting = ?self.config.burst_mode_setting,
            override_disabled = self.hdr_override_disabled,
            auto_frame_count = self.auto_detected_frame_count,
            "HDR+ toggled"
        );

        // Save config only if setting changed
        if config_changed
            && let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save HDR+ setting");
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

        // Reset override when manually changing setting
        self.hdr_override_disabled = false;

        info!(
            setting = ?self.config.burst_mode_setting,
            "HDR+ setting changed (override cleared)"
        );

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save burst mode frame count setting");
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
        self.turn_off_flash_hardware();
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
                    if self.use_hardware_flash() {
                        info!("Flash enabled - turning on hardware flash before capture");
                        self.turn_on_flash_hardware();
                    } else {
                        info!("Flash enabled - showing screen flash before capture");
                        self.flash_active = true;
                    }
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
        // Zoom in by 0.1x, max 10x
        let new_zoom = (self.zoom_level + 0.1).min(10.0);
        if (new_zoom - self.zoom_level).abs() > 0.001 {
            self.zoom_level = new_zoom;
            debug!(zoom = self.zoom_level, "Zoom in");
        }
        Task::none()
    }

    pub(crate) fn handle_zoom_out(&mut self) -> Task<cosmic::Action<Message>> {
        // Zoom out by 0.1x, min 1.0x
        let new_zoom = (self.zoom_level - 0.1).max(1.0);
        if (new_zoom - self.zoom_level).abs() > 0.001 {
            self.zoom_level = new_zoom;
            debug!(zoom = self.zoom_level, "Zoom out");
        }
        Task::none()
    }

    pub(crate) fn handle_reset_zoom(&mut self) -> Task<cosmic::Action<Message>> {
        if (self.zoom_level - 1.0).abs() > 0.001 {
            self.zoom_level = 1.0;
            debug!("Zoom reset to 1.0");
        }
        Task::none()
    }

    pub(crate) fn handle_pinch_zoom(&mut self, level: f32) -> Task<cosmic::Action<Message>> {
        let new_zoom = level.clamp(1.0, 10.0);
        if (new_zoom - self.zoom_level).abs() > 0.001 {
            self.zoom_level = new_zoom;
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
                self.last_media_path = Some(path.clone());
                return Task::done(cosmic::Action::App(Message::RefreshGalleryThumbnail));
            }
            Err(err) => {
                let expected_dir = crate::app::get_photo_directory(&self.config.save_folder_name);
                error!(
                    error = %err,
                    expected_directory = %expected_dir.display(),
                    "Failed to save photo. This may be caused by: \
                     1) Insufficient disk space, \
                     2) Missing write permissions to the Pictures directory, \
                     3) Flatpak sandbox restrictions (ensure xdg-pictures access is granted), \
                     4) XDG_PICTURES_DIR pointing to an inaccessible location"
                );
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
                self.last_media_path = Some(path.clone());
                return Task::done(cosmic::Action::App(Message::RefreshGalleryThumbnail));
            }
            Err(err) => {
                let expected_dir = crate::app::get_photo_directory(&self.config.save_folder_name);
                error!(
                    error = %err,
                    expected_directory = %expected_dir.display(),
                    "Failed to save recording. This may be caused by: \
                     1) Insufficient disk space, \
                     2) Missing write permissions to the Pictures directory, \
                     3) Flatpak sandbox restrictions (ensure xdg-pictures access is granted), \
                     4) XDG_PICTURES_DIR pointing to an inaccessible location"
                );
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
        let save_dir = crate::app::get_video_directory(&self.config.save_folder_name);
        let output_path = save_dir.join(&filename);

        info!(
            device = %camera.path,
            width = format.width,
            height = format.height,
            fps = ?format.framerate,
            output = %output_path.display(),
            "Starting video recording"
        );

        let sensor_rotation = camera.rotation;
        let width = format.width;
        let height = format.height;
        let framerate = format.framerate.map(|f| f.as_int()).unwrap_or(30);

        // Only get audio device if audio recording is enabled in settings
        let audio_device = if self.config.record_audio {
            self.available_audio_devices
                .get(self.current_audio_device_index)
                .map(|dev| dev.node_name.clone())
        } else {
            None
        };

        let selected_encoder = self
            .available_video_encoders
            .get(self.current_video_encoder_index)
            .cloned();

        let bitrate_kbps = self.config.bitrate_preset.bitrate_kbps(width, height);

        // Check if we're using the libcamera backend — if so, use appsrc pipeline
        let use_appsrc =
            self.config.backend == crate::backends::camera::types::CameraBackendType::Libcamera;

        if use_appsrc {
            // For the appsrc pipeline, use the actual viewfinder frame dimensions
            // (not active_format, which may be the raw/Bayer stream resolution).
            let (appsrc_width, appsrc_height) = self
                .current_frame
                .as_ref()
                .map(|f| (f.width, f.height))
                .unwrap_or((width, height));
            let appsrc_bitrate = self
                .config
                .bitrate_preset
                .bitrate_kbps(appsrc_width, appsrc_height);
            return self.start_appsrc_recording(AppsrcRecordingConfig {
                width: appsrc_width,
                height: appsrc_height,
                framerate,
                format: format.clone(),
                output_path,
                sensor_rotation,
                audio_device,
                selected_encoder,
                bitrate_kbps: appsrc_bitrate,
            });
        }

        // --- PipeWire path (existing code) ---
        let device_path = camera.path.clone();
        let metadata_path = camera.metadata_path.clone();
        let pixel_format = format.pixel_format.clone();

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let path_for_message = output_path.display().to_string();

        // Create audio levels handle upfront so the UI can read it immediately.
        // The recorder is created in the async task to avoid blocking the UI thread.
        let audio_levels: crate::pipelines::video::SharedAudioLevels = Default::default();
        self.recording = RecordingState::start(
            path_for_message.clone(),
            stop_tx,
            Some(audio_levels.clone()),
        );

        let recording_task = Task::perform(
            async move {
                let recorder = tokio::task::spawn_blocking(move || {
                    use crate::pipelines::video::{
                        AudioChannels, AudioQuality, EncoderConfig, PipeWireRecorderConfig,
                        RecorderConfig, VideoQuality, VideoRecorder,
                    };

                    let config = EncoderConfig {
                        video_quality: VideoQuality::High,
                        audio_quality: AudioQuality::High,
                        audio_channels: AudioChannels::Mono,
                        width,
                        height,
                        bitrate_override_kbps: Some(bitrate_kbps),
                    };

                    let recorder = VideoRecorder::new(PipeWireRecorderConfig {
                        base: RecorderConfig {
                            width,
                            height,
                            framerate,
                            output_path: output_path.clone(),
                            encoder_config: config,
                            enable_audio: audio_device.is_some(),
                            audio_device: audio_device.as_deref(),
                            encoder_info: selected_encoder.as_ref(),
                            rotation: sensor_rotation,
                            audio_levels,
                        },
                        device_path: &device_path,
                        metadata_path: metadata_path.as_deref(),
                        pixel_format: &pixel_format,
                        preview_sender: None,
                    })?;

                    recorder.start()?;
                    let path = output_path.display().to_string();
                    Ok::<_, String>((recorder, path))
                })
                .await
                .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))?;

                let (recorder, path) = recorder;
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

    /// Start recording using the appsrc pipeline (libcamera backend).
    ///
    /// Frames from the native capture thread are forwarded via an mpsc channel
    /// into a GStreamer appsrc encoding pipeline. The preview continues
    /// uninterrupted since we reuse the same frames.
    fn start_appsrc_recording(
        &mut self,
        config: AppsrcRecordingConfig,
    ) -> Task<cosmic::Action<Message>> {
        use crate::backends::camera::types::PixelFormat;

        let AppsrcRecordingConfig {
            width,
            height,
            framerate,
            format,
            output_path,
            sensor_rotation,
            audio_device,
            selected_encoder,
            bitrate_kbps,
        } = config;

        // Determine pixel format for the appsrc pipeline
        let pixel_format = self
            .current_frame
            .as_ref()
            .map(|f| f.format)
            .unwrap_or_else(|| {
                // Fallback: parse from format string
                PixelFormat::from_gst_format(&format.pixel_format).unwrap_or(PixelFormat::I420)
            });

        info!(
            pixel_format = ?pixel_format,
            "Using appsrc recording pipeline (libcamera backend)"
        );

        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(15);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        let path_for_message = output_path.display().to_string();

        // Direct path: capture thread → appsrc (bypasses UI thread entirely)
        if let Some(ref manager) = self.backend_manager {
            manager.set_recording_sender(Some(frame_tx));
        }

        // Create audio levels handle upfront so the UI can read it immediately.
        // The recorder is created in the async task to avoid blocking the UI thread.
        let audio_levels: crate::pipelines::video::SharedAudioLevels = Default::default();
        self.recording = RecordingState::start(
            path_for_message.clone(),
            stop_tx,
            Some(audio_levels.clone()),
        );

        let backend_manager = self.backend_manager.clone();

        let recording_task = Task::perform(
            async move {
                // Capture runtime handle — spawn_blocking doesn't propagate the
                // tokio context, but the appsrc pusher needs it for tokio::spawn.
                let rt_handle = tokio::runtime::Handle::current();

                let recorder = tokio::task::spawn_blocking(move || {
                    // Enter the runtime context so tokio::spawn works inside
                    // new_from_appsrc (used by the appsrc pusher task).
                    let _guard = rt_handle.enter();

                    use crate::pipelines::video::{
                        AppsrcRecorderConfig, AudioChannels, AudioQuality, EncoderConfig,
                        RecorderConfig, VideoQuality, VideoRecorder,
                    };

                    // Use Low quality for appsrc path (x264 veryfast preset) —
                    // ARM devices can't encode 1080p in real-time with slower presets.
                    let config = EncoderConfig {
                        video_quality: VideoQuality::Low,
                        audio_quality: AudioQuality::High,
                        audio_channels: AudioChannels::Mono,
                        width,
                        height,
                        bitrate_override_kbps: Some(bitrate_kbps),
                    };

                    let recorder = VideoRecorder::new_from_appsrc(
                        AppsrcRecorderConfig {
                            base: RecorderConfig {
                                width,
                                height,
                                framerate,
                                output_path: output_path.clone(),
                                encoder_config: config,
                                enable_audio: audio_device.is_some(),
                                audio_device: audio_device.as_deref(),
                                encoder_info: selected_encoder.as_ref(),
                                rotation: sensor_rotation,
                                audio_levels,
                            },
                            pixel_format,
                        },
                        frame_rx,
                    )?;

                    recorder.start()?;
                    let path = output_path.display().to_string();
                    Ok::<_, String>((recorder, path))
                })
                .await
                .unwrap_or_else(|e| Err(format!("Task join error: {}", e)))?;

                let (recorder, path) = recorder;

                // Wait for stop signal
                let _ = stop_rx.await;

                // Clear the direct recording sender — drops the Sender, closing
                // the channel so the appsrc pusher sees None and sends EOS.
                if let Some(ref manager) = backend_manager {
                    manager.set_recording_sender(None);
                }

                // Give a brief moment for EOS to propagate before stopping the pipeline.
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

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

        // Start a short blur transition (200ms) after stream restarts
        // This keeps the last frame blurred until new frames arrive, then fades out smoothly
        // Don't disable UI since capture is complete
        let _ = self.transition_state.start_with_duration(200, false);

        // Restart the camera stream after HDR+ processing
        // The stream was stopped when processing began to free GPU resources.
        // Increment the restart counter to change the subscription ID and trigger restart.
        // Create a new cancel flag so the new subscription isn't immediately cancelled.
        info!("Restarting camera stream after HDR+ processing");
        self.camera_stream_restart_counter = self.camera_stream_restart_counter.wrapping_add(1);
        self.camera_cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

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
        ProgressCallback, SaveOutputParams, export_burst_frames_dng, process_burst_mode,
        save_output,
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
    let rotation = config.rotation;

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

    // Save first frame as reference (before HDR+ processing)
    if let Some(first_frame) = frames.first()
        && let Err(e) = save_first_burst_frame(
            first_frame,
            &save_dir,
            crop_rect,
            encoding_format,
            &camera_metadata,
            filter,
            rotation,
        )
        .await
    {
        warn!(error = %e, "Failed to save first burst frame");
        // Continue with HDR+ processing even if first frame save fails
    }

    // Create progress callback that updates the atomic counter
    let progress_callback: ProgressCallback = Arc::new(move |progress: f32| {
        let progress_int = (progress * 1000.0) as u32;
        progress_atomic.store(progress_int, std::sync::atomic::Ordering::Relaxed);
    });

    // Process using the unified GPU pipeline with progress reporting
    let merged = process_burst_mode(frames, config, Some(progress_callback)).await?;

    // Save output with optional crop, filter, rotation, and selected encoding format
    let output_path = save_output(
        &merged,
        SaveOutputParams {
            output_dir: save_dir,
            crop_rect,
            encoding_format,
            camera_metadata,
            filter: Some(filter),
            rotation,
            filename_suffix: Some("_HDR+"),
        },
    )
    .await?;

    info!(path = %output_path.display(), "Burst mode photo saved");
    Ok(output_path.display().to_string())
}

/// Save the first frame of a burst as a separate file for comparison
async fn save_first_burst_frame(
    frame: &crate::backends::camera::types::CameraFrame,
    save_dir: &std::path::Path,
    crop_rect: Option<(u32, u32, u32, u32)>,
    encoding_format: crate::pipelines::photo::EncodingFormat,
    camera_metadata: &crate::pipelines::photo::CameraMetadata,
    filter: crate::app::FilterType,
    rotation: crate::backends::camera::types::SensorRotation,
) -> Result<PathBuf, String> {
    use crate::pipelines::photo::burst_mode::{MergedFrame, SaveOutputParams, save_output};

    // Convert CameraFrame to MergedFrame for save_output
    let merged = MergedFrame {
        data: frame.data.to_vec(),
        width: frame.width,
        height: frame.height,
    };

    // Reuse save_output with no filename suffix (plain IMG_{timestamp})
    let path = save_output(
        &merged,
        SaveOutputParams {
            output_dir: save_dir.to_path_buf(),
            crop_rect,
            encoding_format,
            camera_metadata: camera_metadata.clone(),
            filter: Some(filter),
            rotation,
            filename_suffix: None, // No suffix for first frame
        },
    )
    .await?;

    info!(path = %path.display(), "First burst frame saved for comparison");
    Ok(path)
}
