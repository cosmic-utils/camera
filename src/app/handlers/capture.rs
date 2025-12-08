// SPDX-License-Identifier: GPL-3.0-only

//! Capture operations handlers
//!
//! Handles photo capture, video recording, flash, zoom, and timer functionality.

use crate::app::state::{AppModel, CameraMode, Message, RecordingState};
use cosmic::Task;
use std::sync::Arc;
use tracing::{debug, error, info};

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

    /// Capture the current frame as a photo with the selected filter and zoom
    pub(crate) fn capture_photo(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(frame) = &self.current_frame else {
            info!("No frame available to capture");
            return Task::none();
        };

        info!("Capturing photo...");
        self.is_capturing = true;

        let frame_arc = Arc::clone(frame);
        let save_dir = crate::app::get_photo_directory();
        let filter_type = self.selected_filter;
        let zoom_level = self.zoom_level;

        // Calculate crop rectangle based on aspect ratio setting
        let crop_rect = self.photo_aspect_ratio.crop_rect(frame.width, frame.height);
        let crop_rect = if self.photo_aspect_ratio == crate::app::state::PhotoAspectRatio::Native {
            None
        } else {
            Some(crop_rect)
        };

        let save_task = Task::perform(
            async move {
                use crate::pipelines::photo::{
                    EncodingFormat, EncodingQuality, PhotoPipeline, PostProcessingConfig,
                };
                let mut config = PostProcessingConfig::default();
                config.filter_type = filter_type;
                config.crop_rect = crop_rect;
                config.zoom_level = zoom_level;
                let pipeline =
                    PhotoPipeline::with_config(config, EncodingFormat::Jpeg, EncodingQuality::High);
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
}
