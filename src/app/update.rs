// SPDX-License-Identifier: MPL-2.0

//! Message update handling
//!
//! This module handles all application messages by routing them to focused handler methods.
//! The main `update()` function acts as a dispatcher, while specific handlers contain
//! the actual business logic for each message category.
//!
//! # Handler Categories
//!
//! - **UI Navigation**: Context pages, pickers, theatre mode
//! - **Camera Control**: Camera selection, frame handling, transitions
//! - **Format Selection**: Resolution, framerate, codec selection
//! - **Capture Operations**: Photo capture, video recording
//! - **Gallery**: Thumbnail loading, opening gallery
//! - **Filters**: Filter selection
//! - **Settings**: Configuration, device selection
//! - **System**: Bug reports, recovery

use crate::app::state::{AppModel, CameraMode, FilterType, Message, RecordingState};
use crate::app::utils::{parse_codec, parse_resolution};
use cosmic::Task;
use cosmic::cosmic_config::CosmicConfigEntry;
use std::sync::Arc;
use tracing::{error, info};

impl AppModel {
    /// Main message handler - routes messages to appropriate handler methods.
    ///
    /// This dispatcher pattern keeps the main update function clean and makes
    /// it easy to find the handling code for any message type.
    pub fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            // ===== UI Navigation =====
            Message::LaunchUrl(url) => self.handle_launch_url(url),
            Message::ToggleContextPage(page) => self.handle_toggle_context_page(page),
            Message::ToggleFormatPicker => self.handle_toggle_format_picker(),
            Message::CloseFormatPicker => self.handle_close_format_picker(),
            Message::ToggleFilterPicker => self.handle_toggle_filter_picker(),
            Message::CloseFilterPicker => self.handle_close_filter_picker(),
            Message::ToggleTheatreMode => self.handle_toggle_theatre_mode(),
            Message::TheatreShowUI => self.handle_theatre_show_ui(),
            Message::TheatreHideUI => self.handle_theatre_hide_ui(),
            Message::ToggleBitrateInfo => self.handle_toggle_bitrate_info(),
            Message::FilterPickerScroll(delta) => self.handle_filter_picker_scroll(delta),

            // ===== Camera Control =====
            Message::SwitchCamera => self.handle_switch_camera(),
            Message::SelectCamera(index) => self.handle_select_camera(index),
            Message::CameraFrame(frame) => self.handle_camera_frame(frame),
            Message::CamerasInitialized(cameras, index, formats) => {
                self.handle_cameras_initialized(cameras, index, formats)
            }
            Message::CameraListChanged(cameras) => self.handle_camera_list_changed(cameras),
            Message::StartCameraTransition => self.handle_start_camera_transition(),
            Message::ClearTransitionBlur => self.handle_clear_transition_blur(),
            Message::ToggleMirrorPreview => self.handle_toggle_mirror_preview(),

            // ===== Format Selection =====
            Message::SetMode(mode) => self.handle_set_mode(mode),
            Message::SelectMode(index) => self.handle_select_mode(index),
            Message::SelectPixelFormat(format) => self.handle_select_pixel_format(format),
            Message::SelectResolution(resolution) => self.handle_select_resolution(resolution),
            Message::SelectFramerate(framerate) => self.handle_select_framerate(framerate),
            Message::SelectCodec(codec) => self.handle_select_codec(codec),
            Message::PickerSelectResolution(width) => self.handle_picker_select_resolution(width),
            Message::PickerSelectFormat(index) => self.handle_picker_select_format(index),
            Message::SelectBitratePreset(index) => self.handle_select_bitrate_preset(index),

            // ===== Capture Operations =====
            Message::Capture => self.handle_capture(),
            Message::ToggleFlash => self.handle_toggle_flash(),
            Message::FlashComplete => self.handle_flash_complete(),
            Message::PhotoSaved(result) => self.handle_photo_saved(result),
            Message::ClearCaptureAnimation => self.handle_clear_capture_animation(),
            Message::ToggleRecording => self.handle_toggle_recording(),
            Message::RecordingStarted(path) => self.handle_recording_started(path),
            Message::RecordingStopped(result) => self.handle_recording_stopped(result),
            Message::UpdateRecordingDuration => self.handle_update_recording_duration(),
            Message::StartRecordingAfterDelay => self.handle_start_recording_after_delay(),

            // ===== Gallery =====
            Message::OpenGallery => self.handle_open_gallery(),
            Message::RefreshGalleryThumbnail => self.handle_refresh_gallery_thumbnail(),
            Message::GalleryThumbnailLoaded(data) => self.handle_gallery_thumbnail_loaded(data),

            // ===== Filters =====
            Message::SelectFilter(filter) => self.handle_select_filter(filter),

            // ===== Settings =====
            Message::UpdateConfig(config) => self.handle_update_config(config),
            Message::SelectAudioDevice(index) => self.handle_select_audio_device(index),
            Message::SelectVideoEncoder(index) => self.handle_select_video_encoder(index),

            // ===== System & Recovery =====
            Message::CameraRecoveryStarted {
                attempt,
                max_attempts,
            } => self.handle_camera_recovery_started(attempt, max_attempts),
            Message::CameraRecoverySucceeded => self.handle_camera_recovery_succeeded(),
            Message::CameraRecoveryFailed(error) => self.handle_camera_recovery_failed(error),
            Message::AudioRecoveryStarted {
                attempt,
                max_attempts,
            } => self.handle_audio_recovery_started(attempt, max_attempts),
            Message::AudioRecoverySucceeded => self.handle_audio_recovery_succeeded(),
            Message::AudioRecoveryFailed(error) => self.handle_audio_recovery_failed(error),
            Message::GenerateBugReport => self.handle_generate_bug_report(),
            Message::BugReportGenerated(result) => self.handle_bug_report_generated(result),
            Message::ShowBugReport => self.handle_show_bug_report(),

            // ===== QR Code Detection =====
            Message::ToggleQrDetection => self.handle_toggle_qr_detection(),
            Message::QrDetectionsUpdated(detections) => {
                self.handle_qr_detections_updated(detections)
            }
            Message::QrOpenUrl(url) => self.handle_qr_open_url(url),
            Message::QrConnectWifi {
                ssid,
                password,
                security,
                hidden,
            } => self.handle_qr_connect_wifi(ssid, password, security, hidden),
            Message::QrCopyText(text) => self.handle_qr_copy_text(text),
            Message::Noop => Task::none(),
        }
    }

    // =========================================================================
    // UI Navigation Handlers
    // =========================================================================

    fn handle_launch_url(&self, url: String) -> Task<cosmic::Action<Message>> {
        match open::that_detached(&url) {
            Ok(()) => {}
            Err(err) => {
                error!(url = %url, error = %err, "Failed to open URL");
            }
        }
        Task::none()
    }

    fn handle_toggle_context_page(
        &mut self,
        context_page: crate::app::state::ContextPage,
    ) -> Task<cosmic::Action<Message>> {
        if self.context_page == context_page {
            self.core.window.show_context = !self.core.window.show_context;
        } else {
            self.context_page = context_page;
            self.core.window.show_context = true;
        }
        Task::none()
    }

    fn handle_toggle_format_picker(&mut self) -> Task<cosmic::Action<Message>> {
        self.format_picker_visible = !self.format_picker_visible;
        if self.format_picker_visible {
            self.picker_selected_resolution = self.active_format.as_ref().map(|f| f.width);
        }
        Task::none()
    }

    fn handle_close_format_picker(&mut self) -> Task<cosmic::Action<Message>> {
        self.format_picker_visible = false;
        Task::none()
    }

    fn handle_toggle_filter_picker(&mut self) -> Task<cosmic::Action<Message>> {
        self.filter_picker_visible = !self.filter_picker_visible;
        info!("Filter picker toggled: {}", self.filter_picker_visible);
        Task::none()
    }

    fn handle_close_filter_picker(&mut self) -> Task<cosmic::Action<Message>> {
        self.filter_picker_visible = false;
        info!("Filter picker closed");
        Task::none()
    }

    fn handle_toggle_theatre_mode(&mut self) -> Task<cosmic::Action<Message>> {
        if self.theatre.enabled {
            info!("Exiting theatre mode");
            self.theatre.exit();
        } else {
            info!("Entering theatre mode - UI will hide after 1 second");
            self.theatre.enter();

            return Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                },
                |_| cosmic::Action::App(Message::TheatreHideUI),
            );
        }
        Task::none()
    }

    fn handle_theatre_show_ui(&mut self) -> Task<cosmic::Action<Message>> {
        if self.theatre.enabled {
            info!("Theatre mode: showing UI");
            self.theatre.show_ui();

            return Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                },
                |_| cosmic::Action::App(Message::TheatreHideUI),
            );
        }
        Task::none()
    }

    fn handle_theatre_hide_ui(&mut self) -> Task<cosmic::Action<Message>> {
        if self.theatre.try_hide_ui() {
            info!("Theatre mode: hiding UI");
        }
        Task::none()
    }

    fn handle_toggle_bitrate_info(&mut self) -> Task<cosmic::Action<Message>> {
        self.bitrate_info_visible = !self.bitrate_info_visible;
        info!(visible = self.bitrate_info_visible, "Bitrate info toggled");
        Task::none()
    }

    fn handle_filter_picker_scroll(&mut self, delta: f32) -> Task<cosmic::Action<Message>> {
        self.filter_picker_scroll_offset -= delta;
        if self.filter_picker_scroll_offset < 0.0 {
            self.filter_picker_scroll_offset = 0.0;
        }

        let offset = cosmic::iced_widget::scrollable::AbsoluteOffset {
            x: self.filter_picker_scroll_offset,
            y: 0.0,
        };
        cosmic::iced_widget::scrollable::scroll_to(Self::filter_picker_scrollable_id(), offset)
    }

    // =========================================================================
    // Camera Control Handlers
    // =========================================================================

    fn handle_switch_camera(&mut self) -> Task<cosmic::Action<Message>> {
        info!(
            current_index = self.current_camera_index,
            "Received SwitchCamera message"
        );
        if self.available_cameras.len() > 1 {
            self.current_camera_index =
                (self.current_camera_index + 1) % self.available_cameras.len();
            let camera_name = &self.available_cameras[self.current_camera_index].name;
            info!(new_index = self.current_camera_index, camera = %camera_name, "Switching to camera");

            info!("Setting cancellation flag for camera switch");
            self.camera_cancel_flag
                .store(true, std::sync::atomic::Ordering::Release);
            self.camera_cancel_flag =
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            self.switch_camera_or_mode(self.current_camera_index, self.mode);
            let _ = self.transition_state.start();
        } else {
            info!("Only one camera available, cannot switch");
        }
        Task::none()
    }

    fn handle_select_camera(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index < self.available_cameras.len() {
            info!(index, "Selected camera index");

            let _ = self.transition_state.start();
            self.camera_cancel_flag
                .store(true, std::sync::atomic::Ordering::Release);
            self.camera_cancel_flag =
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            self.current_camera_index = index;
            self.switch_camera_or_mode(index, self.mode);
        }
        Task::none()
    }

    fn handle_camera_frame(
        &mut self,
        frame: Arc<crate::backends::camera::types::CameraFrame>,
    ) -> Task<cosmic::Action<Message>> {
        static FRAME_MSG_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = FRAME_MSG_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if count % 30 == 0 {
            info!(
                message = count,
                width = frame.width,
                height = frame.height,
                bytes = frame.data.len(),
                "CameraFrame message received in update()"
            );
        }

        if let Some(task) = self.transition_state.on_frame_received() {
            self.current_frame = Some(frame);
            return task.map(cosmic::Action::App);
        }

        self.current_frame = Some(frame);
        Task::none()
    }

    fn handle_cameras_initialized(
        &mut self,
        cameras: Vec<crate::backends::camera::types::CameraDevice>,
        camera_index: usize,
        formats: Vec<crate::backends::camera::types::CameraFormat>,
    ) -> Task<cosmic::Action<Message>> {
        info!(
            count = cameras.len(),
            camera_index, "Cameras initialized asynchronously"
        );

        self.available_cameras = cameras;
        self.current_camera_index = camera_index;
        self.available_formats = formats.clone();

        self.camera_dropdown_options = self
            .available_cameras
            .iter()
            .map(|cam| {
                cam.name
                    .strip_suffix(" (V4L2)")
                    .unwrap_or(&cam.name)
                    .to_string()
            })
            .collect();

        self.active_format = {
            info!("Photo mode: selecting maximum resolution");
            crate::app::format_picker::preferences::select_max_resolution_format(&formats)
        };

        self.update_mode_options();
        self.update_resolution_options();
        self.update_pixel_format_options();
        self.update_framerate_options();
        self.update_codec_options();

        info!("Camera initialization complete, preview will start");
        Task::none()
    }

    fn handle_camera_list_changed(
        &mut self,
        new_cameras: Vec<crate::backends::camera::types::CameraDevice>,
    ) -> Task<cosmic::Action<Message>> {
        info!(
            old_count = self.available_cameras.len(),
            new_count = new_cameras.len(),
            "Camera list changed (hotplug event)"
        );

        let current_camera_still_available =
            if let Some(current) = self.available_cameras.get(self.current_camera_index) {
                new_cameras
                    .iter()
                    .any(|c| c.path == current.path && c.name == current.name)
            } else {
                false
            };

        self.available_cameras = new_cameras.clone();
        self.camera_dropdown_options = self
            .available_cameras
            .iter()
            .map(|cam| {
                cam.name
                    .strip_suffix(" (V4L2)")
                    .unwrap_or(&cam.name)
                    .to_string()
            })
            .collect();

        if !current_camera_still_available {
            if new_cameras.is_empty() {
                error!("Current camera disconnected and no other cameras available");
                self.current_camera_index = 0;
                self.available_formats.clear();
                self.active_format = None;
                self.update_mode_options();
                self.update_resolution_options();
                self.update_pixel_format_options();
                self.update_framerate_options();
                self.update_codec_options();
                self.camera_cancel_flag
                    .store(true, std::sync::atomic::Ordering::Release);
            } else {
                info!("Current camera disconnected, switching to first available camera");
                self.current_camera_index = 0;

                return Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        0
                    },
                    |index| cosmic::Action::App(Message::SelectCamera(index)),
                );
            }
        } else if let Some(current) = self.available_cameras.get(self.current_camera_index) {
            if let Some(new_index) = new_cameras
                .iter()
                .position(|c| c.path == current.path && c.name == current.name)
            {
                self.current_camera_index = new_index;
            }
        }
        Task::none()
    }

    fn handle_start_camera_transition(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Starting camera transition with blur effect");
        let _ = self.transition_state.start();
        Task::none()
    }

    fn handle_clear_transition_blur(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Clearing transition blur effect");
        self.transition_state.clear();
        Task::none()
    }

    fn handle_toggle_mirror_preview(&mut self) -> Task<cosmic::Action<Message>> {
        self.config.mirror_preview = !self.config.mirror_preview;
        info!(
            mirror_preview = self.config.mirror_preview,
            "Mirror preview toggled"
        );

        if let Some(handler) = self.config_handler.as_ref() {
            if let Err(err) = self.config.write_entry(handler) {
                error!(?err, "Failed to save mirror preview setting");
            }
        }
        Task::none()
    }

    // =========================================================================
    // Format Selection Handlers
    // =========================================================================

    fn handle_set_mode(&mut self, mode: CameraMode) -> Task<cosmic::Action<Message>> {
        if self.mode == mode {
            return Task::none();
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
        self.switch_camera_or_mode(self.current_camera_index, mode);
        Task::none()
    }

    fn handle_select_mode(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
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
        }
        Task::none()
    }

    fn handle_select_pixel_format(
        &mut self,
        pixel_format: String,
    ) -> Task<cosmic::Action<Message>> {
        info!(pixel_format = %pixel_format, "Switching to pixel format");
        self.change_pixel_format(pixel_format);
        let _ = self.transition_state.start();
        Task::none()
    }

    fn handle_select_resolution(
        &mut self,
        resolution_str: String,
    ) -> Task<cosmic::Action<Message>> {
        if let Some((width, height)) = parse_resolution(&resolution_str) {
            info!(width, height, "Switching to resolution");
            self.change_resolution(width, height);
            let _ = self.transition_state.start();
        }
        Task::none()
    }

    fn handle_select_framerate(&mut self, framerate_str: String) -> Task<cosmic::Action<Message>> {
        if let Ok(fps) = framerate_str.parse::<u32>() {
            info!(fps, "Switching to framerate");
            self.change_framerate(fps);
            let _ = self.transition_state.start();
        }
        Task::none()
    }

    fn handle_select_codec(&mut self, codec_str: String) -> Task<cosmic::Action<Message>> {
        let pixel_format = parse_codec(&codec_str);
        info!(pixel_format = %pixel_format, "Switching to codec");
        self.change_pixel_format(pixel_format);
        Task::none()
    }

    fn handle_picker_select_resolution(&mut self, width: u32) -> Task<cosmic::Action<Message>> {
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
                matching_formats
                    .iter()
                    .find(|(_, fmt)| fmt.framerate == Some(target_fps))
                    .or_else(|| {
                        matching_formats
                            .iter()
                            .filter(|(_, fmt)| fmt.framerate.is_some())
                            .min_by_key(|(_, fmt)| {
                                let fps = fmt.framerate.unwrap();
                                ((fps as i32) - (target_fps as i32)).abs()
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
                }
                self.save_settings();
                let _ = self.transition_state.start();
            }
        }
        Task::none()
    }

    fn handle_picker_select_format(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index < self.available_formats.len() {
            self.active_format = self.available_formats.get(index).cloned();
            self.format_picker_visible = false;

            if let Some(fmt) = &self.active_format {
                info!(format = %fmt, "Selected format from picker");
            }
            self.save_settings();
            let _ = self.transition_state.start();
        }
        Task::none()
    }

    fn handle_select_bitrate_preset(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index < crate::constants::BitratePreset::ALL.len() {
            let preset = crate::constants::BitratePreset::ALL[index];
            info!(preset = ?preset, "Selected bitrate preset");
            self.config.bitrate_preset = preset;

            if let Some(handler) = self.config_handler.as_ref() {
                if let Err(err) = self.config.write_entry(handler) {
                    error!(?err, "Failed to save bitrate preset setting");
                }
            }
        }
        Task::none()
    }

    // =========================================================================
    // Capture Operations Handlers
    // =========================================================================

    fn handle_capture(&mut self) -> Task<cosmic::Action<Message>> {
        if self.mode == CameraMode::Photo && self.flash_enabled && !self.flash_active {
            info!("Flash enabled - showing flash before capture");
            self.flash_active = true;

            return Task::perform(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                },
                |_| cosmic::Action::App(Message::FlashComplete),
            );
        }

        if let Some(frame) = &self.current_frame {
            info!("Capturing photo...");
            self.is_capturing = true;

            let frame_arc = Arc::clone(frame);
            let save_dir = crate::app::get_photo_directory();
            let filter_type = self.selected_filter;

            let save_task = Task::perform(
                async move {
                    use crate::pipelines::photo::{
                        EncodingFormat, EncodingQuality, PhotoPipeline, PostProcessingConfig,
                    };
                    let mut config = PostProcessingConfig::default();
                    config.filter_type = filter_type;
                    let pipeline = PhotoPipeline::with_config(
                        config,
                        EncodingFormat::Jpeg,
                        EncodingQuality::High,
                    );
                    pipeline
                        .capture_and_save(frame_arc, save_dir)
                        .await
                        .map(|p| p.display().to_string())
                },
                |result| cosmic::Action::App(Message::PhotoSaved(result)),
            );

            let animation_task = Task::perform(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                },
                |_| cosmic::Action::App(Message::ClearCaptureAnimation),
            );

            return Task::batch([save_task, animation_task]);
        } else {
            info!("No frame available to capture");
        }
        Task::none()
    }

    fn handle_toggle_flash(&mut self) -> Task<cosmic::Action<Message>> {
        self.flash_enabled = !self.flash_enabled;
        info!(flash_enabled = self.flash_enabled, "Flash toggled");
        Task::none()
    }

    fn handle_flash_complete(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Flash complete - capturing photo");
        self.flash_active = false;

        if let Some(frame) = &self.current_frame {
            self.is_capturing = true;
            let frame_arc = Arc::clone(frame);
            let save_dir = crate::app::get_photo_directory();
            let filter_type = self.selected_filter;

            let save_task = Task::perform(
                async move {
                    use crate::pipelines::photo::{
                        EncodingFormat, EncodingQuality, PhotoPipeline, PostProcessingConfig,
                    };
                    let mut config = PostProcessingConfig::default();
                    config.filter_type = filter_type;
                    let pipeline = PhotoPipeline::with_config(
                        config,
                        EncodingFormat::Jpeg,
                        EncodingQuality::High,
                    );
                    pipeline
                        .capture_and_save(frame_arc, save_dir)
                        .await
                        .map(|p| p.display().to_string())
                },
                |result| cosmic::Action::App(Message::PhotoSaved(result)),
            );

            let animation_task = Task::perform(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                },
                |_| cosmic::Action::App(Message::ClearCaptureAnimation),
            );

            return Task::batch([save_task, animation_task]);
        }
        Task::none()
    }

    fn handle_photo_saved(
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

    fn handle_clear_capture_animation(&mut self) -> Task<cosmic::Action<Message>> {
        self.is_capturing = false;
        Task::none()
    }

    fn handle_toggle_recording(&mut self) -> Task<cosmic::Action<Message>> {
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

    fn handle_recording_started(&mut self, path: String) -> Task<cosmic::Action<Message>> {
        info!(path = %path, "Recording started successfully");
        Task::perform(
            async {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            },
            |_| cosmic::Action::App(Message::UpdateRecordingDuration),
        )
    }

    fn handle_recording_stopped(
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

    fn handle_update_recording_duration(&mut self) -> Task<cosmic::Action<Message>> {
        if self.recording.is_recording() {
            return Task::perform(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                },
                |_| cosmic::Action::App(Message::UpdateRecordingDuration),
            );
        }
        Task::none()
    }

    fn handle_start_recording_after_delay(&mut self) -> Task<cosmic::Action<Message>> {
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

    // =========================================================================
    // Gallery Handlers
    // =========================================================================

    fn handle_open_gallery(&self) -> Task<cosmic::Action<Message>> {
        let photo_dir = crate::app::get_photo_directory();
        info!(path = %photo_dir.display(), "Opening gallery directory");

        if let Err(e) = open::that(&photo_dir) {
            error!(error = %e, path = %photo_dir.display(), "Failed to open gallery directory");
        } else {
            info!("Gallery opened successfully");
        }
        Task::none()
    }

    fn handle_refresh_gallery_thumbnail(&self) -> Task<cosmic::Action<Message>> {
        let save_dir = crate::app::get_photo_directory();
        Task::perform(
            async move { crate::storage::load_latest_thumbnail(save_dir).await },
            |handle| cosmic::Action::App(Message::GalleryThumbnailLoaded(handle)),
        )
    }

    fn handle_gallery_thumbnail_loaded(
        &mut self,
        data: Option<(cosmic::widget::image::Handle, Arc<Vec<u8>>, u32, u32)>,
    ) -> Task<cosmic::Action<Message>> {
        if let Some((handle, rgba, width, height)) = data {
            self.gallery_thumbnail = Some(handle);
            self.gallery_thumbnail_rgba = Some((rgba, width, height));
        } else {
            self.gallery_thumbnail = None;
            self.gallery_thumbnail_rgba = None;
        }
        Task::none()
    }

    // =========================================================================
    // Filter Handlers
    // =========================================================================

    fn handle_select_filter(&mut self, filter: FilterType) -> Task<cosmic::Action<Message>> {
        self.selected_filter = filter;
        info!("Filter selected: {:?}", filter);
        Task::none()
    }

    // =========================================================================
    // Settings Handlers
    // =========================================================================

    fn handle_update_config(
        &mut self,
        config: crate::config::Config,
    ) -> Task<cosmic::Action<Message>> {
        info!("UpdateConfig received");
        self.config = config;
        Task::none()
    }

    fn handle_select_audio_device(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index < self.available_audio_devices.len() {
            info!(index, "Selected audio device index");
            self.current_audio_device_index = index;
        }
        Task::none()
    }

    fn handle_select_video_encoder(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index < self.available_video_encoders.len() {
            info!(index, encoder = %self.available_video_encoders[index].display_name, "Selected video encoder");
            self.current_video_encoder_index = index;

            self.config.last_video_encoder_index = Some(index);
            if let Some(handler) = self.config_handler.as_ref() {
                if let Err(err) = self.config.write_entry(handler) {
                    error!(?err, "Failed to save encoder selection");
                }
            }
        }
        Task::none()
    }

    // =========================================================================
    // System & Recovery Handlers
    // =========================================================================

    fn handle_camera_recovery_started(
        &self,
        attempt: u32,
        max_attempts: u32,
    ) -> Task<cosmic::Action<Message>> {
        info!(attempt, max_attempts, "Camera backend recovery started");
        Task::none()
    }

    fn handle_camera_recovery_succeeded(&self) -> Task<cosmic::Action<Message>> {
        info!("Camera backend recovery succeeded");
        Task::none()
    }

    fn handle_camera_recovery_failed(&self, error: String) -> Task<cosmic::Action<Message>> {
        error!(error = %error, "Camera backend recovery failed");
        Task::none()
    }

    fn handle_audio_recovery_started(
        &self,
        attempt: u32,
        max_attempts: u32,
    ) -> Task<cosmic::Action<Message>> {
        info!(attempt, max_attempts, "Audio backend recovery started");
        Task::none()
    }

    fn handle_audio_recovery_succeeded(&self) -> Task<cosmic::Action<Message>> {
        info!("Audio backend recovery succeeded");
        Task::none()
    }

    fn handle_audio_recovery_failed(&self, error: String) -> Task<cosmic::Action<Message>> {
        error!(error = %error, "Audio backend recovery failed");
        Task::none()
    }

    fn handle_generate_bug_report(&self) -> Task<cosmic::Action<Message>> {
        info!("Generating bug report...");

        let video_devices = self.available_cameras.clone();
        let audio_devices = self.available_audio_devices.clone();
        let video_encoders = self.available_video_encoders.clone();
        let selected_encoder_index = self.current_video_encoder_index;

        Task::perform(
            async move {
                crate::bug_report::BugReportGenerator::generate(
                    &video_devices,
                    &audio_devices,
                    &video_encoders,
                    selected_encoder_index,
                    None,
                )
                .await
                .map(|p| p.display().to_string())
            },
            |result| cosmic::Action::App(Message::BugReportGenerated(result)),
        )
    }

    fn handle_bug_report_generated(
        &mut self,
        result: Result<String, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(path) => {
                info!(path = %path, "Bug report generated successfully");
                self.last_bug_report_path = Some(path);

                let url = &self.config.bug_report_url;
                if let Err(e) = open::that(url) {
                    error!(error = %e, url = %url, "Failed to open bug report URL");
                } else {
                    info!(url = %url, "Opened bug report URL");
                }
            }
            Err(err) => {
                error!(error = %err, "Failed to generate bug report");
            }
        }
        Task::none()
    }

    fn handle_show_bug_report(&self) -> Task<cosmic::Action<Message>> {
        if let Some(report_path) = &self.last_bug_report_path {
            info!(path = %report_path, "Showing bug report in file manager");
            if let Err(e) = Self::show_in_file_manager(report_path) {
                error!(error = %e, path = %report_path, "Failed to show bug report in file manager");
            }
        }
        Task::none()
    }

    // =========================================================================
    // Helper Functions
    // =========================================================================

    /// Show a file in the file manager with pre-selection
    fn show_in_file_manager(file_path: &str) -> Result<(), String> {
        use std::process::Command;

        let path = std::path::Path::new(file_path);
        let file_uri = format!("file://{}", path.display());

        // Method 1: Try D-Bus FileManager1.ShowItems
        let dbus_result = Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{}", file_uri),
                "string:",
            ])
            .output();

        if let Ok(output) = dbus_result {
            if output.status.success() {
                info!("Opened file manager via D-Bus");
                return Ok(());
            }
        }

        // Method 2: Try file manager-specific commands
        let file_managers = [
            ("nautilus", vec!["--select", file_path]),
            ("dolphin", vec!["--select", file_path]),
            ("nemo", vec![file_path]),
            ("caja", vec![file_path]),
            ("thunar", vec![file_path]),
        ];

        for (fm_name, args) in &file_managers {
            if let Ok(output) = Command::new(fm_name).args(args).spawn() {
                info!(file_manager = fm_name, "Opened file manager");
                drop(output);
                return Ok(());
            }
        }

        // Method 3: Fallback to opening the parent directory
        if let Some(parent) = path.parent() {
            if let Ok(child) = Command::new("xdg-open").arg(parent).spawn() {
                info!("Opened parent directory as fallback");
                drop(child);
                return Ok(());
            }
        }

        Err("Failed to open file manager".to_string())
    }

    // =========================================================================
    // QR Code Detection Handlers
    // =========================================================================

    fn handle_toggle_qr_detection(&mut self) -> Task<cosmic::Action<Message>> {
        self.qr_detection_enabled = !self.qr_detection_enabled;
        info!(enabled = self.qr_detection_enabled, "QR detection toggled");

        // Clear detections when disabling
        if !self.qr_detection_enabled {
            self.qr_detections.clear();
        }

        Task::none()
    }

    fn handle_qr_detections_updated(
        &mut self,
        detections: Vec<crate::app::frame_processor::QrDetection>,
    ) -> Task<cosmic::Action<Message>> {
        let count = detections.len();
        self.qr_detections = detections;
        self.last_qr_detection_time = Some(std::time::Instant::now());

        if count > 0 {
            info!(count, "QR detections updated");
        }

        Task::none()
    }

    fn handle_qr_open_url(&self, url: String) -> Task<cosmic::Action<Message>> {
        info!(url = %url, "Opening URL from QR code");
        match open::that_detached(&url) {
            Ok(()) => {
                info!("URL opened successfully");
            }
            Err(err) => {
                error!(url = %url, error = %err, "Failed to open URL");
            }
        }
        Task::none()
    }

    fn handle_qr_connect_wifi(
        &self,
        ssid: String,
        password: Option<String>,
        security: String,
        hidden: bool,
    ) -> Task<cosmic::Action<Message>> {
        info!(
            ssid = %ssid,
            security = %security,
            hidden,
            has_password = password.is_some(),
            "Connecting to WiFi from QR code"
        );

        // Map security type to nmcli key-mgmt value
        let key_mgmt = match security.to_uppercase().as_str() {
            "OPEN" => "none",
            "WEP" => "wep",
            "WPA/WPA2" | "WPA" | "WPA2" => "wpa-psk",
            "ENTERPRISE" => "wpa-eap",
            "WPA3" => "sae",
            _ => "wpa-psk", // Default to WPA-PSK for unknown types
        };

        Task::perform(
            async move {
                use std::process::Command;

                // First, try to delete any existing connection with this SSID
                // (ignore errors - connection might not exist)
                let _ = Command::new("nmcli")
                    .args(["connection", "delete", &ssid])
                    .output();

                // Build the connection add command
                let mut args = vec![
                    "connection".to_string(),
                    "add".to_string(),
                    "type".to_string(),
                    "wifi".to_string(),
                    "con-name".to_string(),
                    ssid.clone(),
                    "ssid".to_string(),
                    ssid.clone(),
                ];

                // Add security settings based on key management type
                if key_mgmt != "none" {
                    args.push("wifi-sec.key-mgmt".to_string());
                    args.push(key_mgmt.to_string());

                    if let Some(pwd) = &password {
                        if key_mgmt == "wpa-psk" || key_mgmt == "sae" {
                            args.push("wifi-sec.psk".to_string());
                            args.push(pwd.clone());
                        } else if key_mgmt == "wep" {
                            args.push("wifi-sec.wep-key0".to_string());
                            args.push(pwd.clone());
                        }
                    }
                }

                // Handle hidden networks
                if hidden {
                    args.push("wifi.hidden".to_string());
                    args.push("yes".to_string());
                }

                info!(args = ?args, "Creating WiFi connection");

                // Create the connection
                let create_result = Command::new("nmcli").args(&args).output();

                match create_result {
                    Ok(output) => {
                        if !output.status.success() {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            error!(ssid = %ssid, error = %stderr, "Failed to create WiFi connection");
                            return Err(stderr.to_string());
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to execute nmcli");
                        return Err(e.to_string());
                    }
                }

                // Activate the connection
                let activate_result = Command::new("nmcli")
                    .args(["connection", "up", &ssid])
                    .output();

                match activate_result {
                    Ok(output) => {
                        if output.status.success() {
                            info!(ssid = %ssid, "WiFi connection successful");
                            Ok(())
                        } else {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            error!(ssid = %ssid, error = %stderr, "WiFi connection activation failed");
                            Err(stderr.to_string())
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to execute nmcli");
                        Err(e.to_string())
                    }
                }
            },
            |_| cosmic::Action::App(Message::Noop),
        )
    }

    fn handle_qr_copy_text(&self, text: String) -> Task<cosmic::Action<Message>> {
        info!(
            text_length = text.len(),
            "Copying text from QR code to clipboard"
        );

        // Use wl-copy for Wayland or xclip for X11
        Task::perform(
            async move {
                use std::io::Write;
                use std::process::{Command, Stdio};

                // Try wl-copy first (Wayland)
                let wl_result = Command::new("wl-copy")
                    .stdin(Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        if let Some(stdin) = child.stdin.as_mut() {
                            stdin.write_all(text.as_bytes())?;
                        }
                        child.wait()
                    });

                if let Ok(status) = wl_result {
                    if status.success() {
                        info!("Text copied to clipboard via wl-copy");
                        return Ok(());
                    }
                }

                // Fallback to xclip
                let xclip_result = Command::new("xclip")
                    .args(["-selection", "clipboard"])
                    .stdin(Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        if let Some(stdin) = child.stdin.as_mut() {
                            stdin.write_all(text.as_bytes())?;
                        }
                        child.wait()
                    });

                if let Ok(status) = xclip_result {
                    if status.success() {
                        info!("Text copied to clipboard via xclip");
                        return Ok(());
                    }
                }

                error!("Failed to copy text to clipboard - no clipboard tool available");
                Err("No clipboard tool available (tried wl-copy and xclip)".to_string())
            },
            |_| cosmic::Action::App(Message::Noop),
        )
    }
}
