// SPDX-License-Identifier: GPL-3.0-only

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

use crate::app::state::{
    AppModel, CameraMode, FileSource, FilterType, Message, RecordingState, VideoPlaybackCommand,
    VirtualCameraState,
};
use crate::app::utils::{parse_codec, parse_resolution};
use cosmic::Task;
use cosmic::cosmic_config::CosmicConfigEntry;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

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
            Message::ToggleTheatreMode => self.handle_toggle_theatre_mode(),
            Message::TheatreShowUI => self.handle_theatre_show_ui(),
            Message::TheatreHideUI => self.handle_theatre_hide_ui(),
            Message::ToggleBitrateInfo => self.handle_toggle_bitrate_info(),

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
            Message::ToggleVirtualCameraEnabled => self.handle_toggle_virtual_camera_enabled(),

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

            // ===== Virtual Camera =====
            Message::ToggleVirtualCamera => self.handle_toggle_virtual_camera(),
            Message::VirtualCameraStarted => self.handle_virtual_camera_started(),
            Message::VirtualCameraStopped(result) => self.handle_virtual_camera_stopped(result),
            Message::UpdateVirtualCameraDuration => self.handle_update_virtual_camera_duration(),
            Message::OpenVirtualCameraFile => self.handle_open_virtual_camera_file(),
            Message::VirtualCameraFileSelected(file_source) => {
                self.handle_virtual_camera_file_selected(file_source)
            }
            Message::ClearVirtualCameraFile => self.handle_clear_virtual_camera_file(),
            Message::FileSourcePreviewLoaded(frame, duration) => {
                self.handle_file_source_preview_loaded(frame, duration)
            }
            Message::VideoFileProgress(position, duration, progress) => {
                self.handle_video_file_progress(position, duration, progress)
            }
            Message::VideoFileSeek(position) => self.handle_video_file_seek(position),
            Message::VideoSeekPreviewLoaded(frame) => self.handle_video_seek_preview_loaded(frame),
            Message::VideoPreviewPlaybackUpdate(frame, pos, dur, progress) => {
                self.handle_video_preview_playback_update(frame, pos, dur, progress)
            }
            Message::VideoPreviewPlaybackStopped => self.handle_video_preview_playback_stopped(),
            Message::ToggleVideoPlayPause => self.handle_toggle_video_play_pause(),
            Message::StartVideoPreviewPlayback => self.start_video_preview_playback(),

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

        // When in Virtual mode with file source but NOT streaming, skip camera frames
        // (file source preview is shown via FileSourcePreviewLoaded message)
        // When streaming from file source, accept frames (they come from preview subscription)
        if self.mode == CameraMode::Virtual
            && self.virtual_camera_file_source.is_some()
            && !self.virtual_camera.is_file_source()
        {
            // Skip camera frames - file source preview is shown separately
            return Task::none();
        }

        // Send frame to virtual camera if streaming from camera (not file source)
        if self.virtual_camera.is_streaming() && !self.virtual_camera.is_file_source() {
            if !self.virtual_camera.send_frame(Arc::clone(&frame)) {
                debug!("Failed to send frame to virtual camera (channel closed)");
            }
        }

        // Track whether this frame is from a file source (for mirror handling)
        let is_file_source = self.virtual_camera.is_file_source();

        if let Some(task) = self.transition_state.on_frame_received() {
            self.current_frame = Some(frame);
            self.current_frame_is_file_source = is_file_source;
            return task.map(cosmic::Action::App);
        }

        self.current_frame = Some(frame);
        self.current_frame_is_file_source = is_file_source;
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
            // Stop virtual camera streaming if the camera used for streaming is disconnected
            if self.virtual_camera.is_streaming() {
                info!("Camera disconnected during virtual camera streaming, stopping stream");
                if let Some(sender) = self.virtual_camera.take_stop_sender() {
                    let _ = sender.send(());
                }
                self.virtual_camera = VirtualCameraState::Idle;
            }

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

    fn handle_toggle_virtual_camera_enabled(&mut self) -> Task<cosmic::Action<Message>> {
        self.config.virtual_camera_enabled = !self.config.virtual_camera_enabled;
        info!(
            virtual_camera_enabled = self.config.virtual_camera_enabled,
            "Virtual camera feature toggled"
        );

        // If disabling while in Virtual mode, switch to Photo mode
        if !self.config.virtual_camera_enabled && self.mode == CameraMode::Virtual {
            // Stop virtual camera if streaming
            if self.virtual_camera.is_streaming() {
                if let Some(sender) = self.virtual_camera.take_stop_sender() {
                    let _ = sender.send(());
                }
                self.virtual_camera = VirtualCameraState::Idle;
            }
            self.mode = CameraMode::Photo;
        }

        if let Some(handler) = self.config_handler.as_ref() {
            if let Err(err) = self.config.write_entry(handler) {
                error!(?err, "Failed to save virtual camera setting");
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
        self.switch_camera_or_mode(self.current_camera_index, mode);

        // When switching to Virtual mode with a file source, restore the file source preview
        if mode == CameraMode::Virtual {
            if let Some(ref source) = self.virtual_camera_file_source {
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
        }

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

    /// Capture the current frame as a photo with the selected filter
    fn capture_photo(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(frame) = &self.current_frame else {
            info!("No frame available to capture");
            return Task::none();
        };

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

    /// Create a delayed task that sends a message after the specified milliseconds
    fn delay_task(millis: u64, message: Message) -> Task<cosmic::Action<Message>> {
        Task::perform(
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(millis)).await;
                message
            },
            cosmic::Action::App,
        )
    }

    fn handle_capture(&mut self) -> Task<cosmic::Action<Message>> {
        if self.mode == CameraMode::Photo && self.flash_enabled && !self.flash_active {
            info!("Flash enabled - showing flash before capture");
            self.flash_active = true;
            return Self::delay_task(1000, Message::FlashComplete);
        }
        self.capture_photo()
    }

    fn handle_toggle_flash(&mut self) -> Task<cosmic::Action<Message>> {
        self.flash_enabled = !self.flash_enabled;
        info!(flash_enabled = self.flash_enabled, "Flash toggled");
        Task::none()
    }

    fn handle_flash_complete(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Flash complete - capturing photo");
        self.flash_active = false;
        self.capture_photo()
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
        Self::delay_task(1000, Message::UpdateRecordingDuration)
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
            return Self::delay_task(1000, Message::UpdateRecordingDuration);
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
    // Virtual Camera Handlers
    // =========================================================================

    fn handle_toggle_virtual_camera(&mut self) -> Task<cosmic::Action<Message>> {
        if self.virtual_camera.is_streaming() {
            info!("Stopping virtual camera streaming");

            // For video file sources, save the current playback position BEFORE stopping
            // so we can resume from this position later
            if matches!(self.virtual_camera_file_source, Some(FileSource::Video(_))) {
                if let Some((current_position, _, _)) = self.video_file_progress {
                    self.video_preview_seek_position = current_position;
                }
            }

            if let Some(sender) = self.virtual_camera.take_stop_sender() {
                let _ = sender.send(());
            }
            // Set to Idle immediately so UI updates (button state changes)
            // but don't send VirtualCameraStopped - the streaming thread will send it
            // when it actually stops. This avoids duplicate messages.
            self.virtual_camera = VirtualCameraState::Idle;
            return Task::none();
        }

        // Check if we have a file source
        if let Some(file_source) = &self.virtual_camera_file_source {
            return self.start_virtual_camera_from_file(file_source.clone());
        }

        // Start virtual camera from camera
        let Some(format) = &self.active_format else {
            error!("No active format for virtual camera");
            return Task::none();
        };

        let width = format.width;
        let height = format.height;
        let filter_type = self.selected_filter;

        info!(
            width,
            height,
            ?filter_type,
            "Starting virtual camera streaming from camera"
        );

        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel();
        let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel();
        let (filter_tx, mut filter_rx) = tokio::sync::watch::channel(filter_type);
        self.virtual_camera = VirtualCameraState::start(stop_tx, frame_tx, filter_tx);

        // Start the virtual camera streaming on a DEDICATED THREAD
        // This is critical: CPU filtering is blocking and must NOT run on the async executor
        //
        // Use a oneshot channel to communicate completion back to the async task
        // This avoids blocking the executor with handle.join()
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

        // Spawn the dedicated thread immediately (fire-and-forget from thread perspective)
        std::thread::spawn(move || {
            use crate::backends::virtual_camera::VirtualCameraManager;

            // Create and start the virtual camera on this dedicated thread
            let mut manager = VirtualCameraManager::new();
            manager.set_filter(filter_type);

            let result = (|| {
                if let Err(e) = manager.start(width, height) {
                    return Err(format!("Failed to start virtual camera: {}", e));
                }

                info!("Virtual camera started on dedicated thread, processing frames");

                // Process frames until channel closes
                let mut frame_count = 0u64;
                let mut dropped_count = 0u64;

                loop {
                    // Check for filter updates (non-blocking)
                    if filter_rx.has_changed().unwrap_or(false) {
                        let new_filter = *filter_rx.borrow_and_update();
                        manager.set_filter(new_filter);
                        info!(?new_filter, "Virtual camera filter updated");
                    }

                    // Wait for at least one frame (blocking is OK on dedicated thread)
                    let first_frame = match frame_rx.blocking_recv() {
                        Some(f) => f,
                        None => {
                            info!("Frame channel closed, stopping virtual camera");
                            break;
                        }
                    };

                    // Drain any additional frames, keeping only the latest
                    let mut latest_frame = first_frame;
                    while let Ok(newer_frame) = frame_rx.try_recv() {
                        latest_frame = newer_frame;
                        dropped_count += 1;
                    }

                    frame_count += 1;
                    if frame_count % 30 == 0 {
                        debug!(
                            frame = frame_count,
                            dropped = dropped_count,
                            "Processing virtual camera frame"
                        );
                    }

                    // CPU filtering happens here - blocking is fine on dedicated thread
                    if let Err(e) = manager.push_frame(&latest_frame) {
                        warn!(?e, "Failed to push frame to virtual camera");
                    }
                }

                info!("Shutting down virtual camera");

                if let Err(e) = manager.stop() {
                    warn!(?e, "Error stopping virtual camera");
                }

                Ok(())
            })();

            // Signal completion to the async task (ignore if receiver dropped)
            let _ = done_tx.send(result);
        });

        // Create async task that waits for thread completion WITHOUT blocking
        let streaming_task = Task::perform(
            async move {
                // This awaits the oneshot channel - doesn't block the executor!
                match done_rx.await {
                    Ok(result) => result,
                    Err(_) => Err("Virtual camera thread terminated unexpectedly".to_string()),
                }
            },
            |result| cosmic::Action::App(Message::VirtualCameraStopped(result)),
        );

        let start_signal = Task::done(cosmic::Action::App(Message::VirtualCameraStarted));

        Task::batch([start_signal, streaming_task])
    }

    /// Start virtual camera streaming from a file source (image or video)
    fn start_virtual_camera_from_file(
        &mut self,
        file_source: FileSource,
    ) -> Task<cosmic::Action<Message>> {
        // Stop any preview playback before starting streaming
        self.stop_video_preview_playback();

        let filter_type = self.selected_filter;
        let is_video = matches!(file_source, FileSource::Video(_));

        info!(
            ?file_source,
            ?filter_type,
            "Starting virtual camera from file source"
        );

        // Create channels
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (frame_tx, _frame_rx) = tokio::sync::mpsc::unbounded_channel();
        let (filter_tx, mut filter_rx) = tokio::sync::watch::channel(filter_type);

        // Create preview channel for sending frames back to UI
        let (preview_tx, preview_rx) = tokio::sync::mpsc::unbounded_channel();
        self.file_source_preview_receiver =
            Some(std::sync::Arc::new(tokio::sync::Mutex::new(preview_rx)));

        // Create progress channel for video files
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel::<(f64, f64, f64)>();

        // Create playback control channel for video files
        let (control_tx, control_rx) =
            tokio::sync::mpsc::unbounded_channel::<VideoPlaybackCommand>();

        // Use start_file_source to mark this as file source streaming
        self.virtual_camera = VirtualCameraState::start_file_source(stop_tx, frame_tx, filter_tx);

        // For video files, keep the current progress (with stored seek position) until
        // the streaming thread sends actual progress updates. This prevents the slider
        // from jumping to 0 briefly before showing the correct position.
        // For non-video files, clear the progress.
        // Note: We preserve video_file_paused state so streaming respects play/pause.
        if !is_video {
            self.video_file_progress = None;
        }

        // Store control channel for video files
        if is_video {
            self.video_playback_control_tx = Some(control_tx);
        } else {
            self.video_playback_control_tx = None;
        }

        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

        // Get the stored seek position and paused state to apply when streaming starts
        let initial_seek_position = self.video_preview_seek_position;
        let initial_paused = self.video_file_paused;

        // Spawn dedicated thread for file source streaming
        std::thread::spawn(move || {
            let result = match file_source {
                FileSource::Image(path) => Self::stream_image_to_virtual_camera(
                    &path,
                    filter_type,
                    &mut filter_rx,
                    stop_rx,
                    preview_tx,
                ),
                FileSource::Video(path) => Self::stream_video_to_virtual_camera(
                    &path,
                    filter_type,
                    &mut filter_rx,
                    stop_rx,
                    preview_tx,
                    progress_tx,
                    control_rx,
                    initial_seek_position,
                    initial_paused,
                ),
            };

            let _ = done_tx.send(result);
        });

        let streaming_task = Task::perform(
            async move {
                match done_rx.await {
                    Ok(result) => result,
                    Err(_) => Err("Virtual camera thread terminated unexpectedly".to_string()),
                }
            },
            |result| cosmic::Action::App(Message::VirtualCameraStopped(result)),
        );

        let start_signal = Task::done(cosmic::Action::App(Message::VirtualCameraStarted));

        // For video files, also spawn a task to receive progress updates
        if is_video {
            let progress_task = Task::run(
                futures::stream::unfold(progress_rx, |mut rx| async move {
                    rx.recv().await.map(|(pos, dur, progress)| {
                        (Message::VideoFileProgress(pos, dur, progress), rx)
                    })
                }),
                cosmic::Action::App,
            );
            Task::batch([start_signal, streaming_task, progress_task])
        } else {
            Task::batch([start_signal, streaming_task])
        }
    }

    /// Stream an image file to the virtual camera at ~30fps
    fn stream_image_to_virtual_camera(
        path: &std::path::Path,
        initial_filter: FilterType,
        filter_rx: &mut tokio::sync::watch::Receiver<FilterType>,
        mut stop_rx: tokio::sync::oneshot::Receiver<()>,
        preview_tx: tokio::sync::mpsc::UnboundedSender<
            Arc<crate::backends::camera::types::CameraFrame>,
        >,
    ) -> Result<(), String> {
        use crate::backends::virtual_camera::{VirtualCameraManager, load_image_as_frame};

        // Load the image
        let frame =
            load_image_as_frame(path).map_err(|e| format!("Failed to load image: {}", e))?;

        let width = frame.width;
        let height = frame.height;

        // Create and start virtual camera manager
        let mut manager = VirtualCameraManager::new();
        manager.set_filter(initial_filter);
        // File sources should not be mirrored - output exactly as the file content

        if let Err(e) = manager.start(width, height) {
            return Err(format!("Failed to start virtual camera: {}", e));
        }

        info!(width, height, "Streaming image to virtual camera");

        // Stream at approximately 30fps
        use crate::constants::virtual_camera as vc_timing;
        let frame_duration = vc_timing::IMAGE_STREAM_FRAME_DURATION;
        let mut frame_count = 0u64;

        // Wrap frame in Arc for preview
        let frame_arc = Arc::new(frame);

        loop {
            // Check for stop signal
            match stop_rx.try_recv() {
                Ok(()) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    info!("Stop signal received, stopping image stream");
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }

            // Check for filter updates
            if filter_rx.has_changed().unwrap_or(false) {
                let new_filter = *filter_rx.borrow_and_update();
                manager.set_filter(new_filter);
                info!(?new_filter, "Virtual camera filter updated");
            }

            // Push the frame to virtual camera (file sources are never mirrored - they
            // display content as-is, unlike camera selfie preview which mirrors)
            if let Err(e) = manager.push_frame(&frame_arc) {
                warn!(?e, "Failed to push frame to virtual camera");
            }

            // Send frame to preview (ignore errors if UI is not consuming fast enough)
            let _ = preview_tx.send(Arc::clone(&frame_arc));

            frame_count += 1;
            if frame_count % 30 == 0 {
                debug!(frame = frame_count, "Image streaming");
            }

            std::thread::sleep(frame_duration);
        }

        if let Err(e) = manager.stop() {
            warn!(?e, "Error stopping virtual camera");
        }

        Ok(())
    }

    /// Stream a video file to the virtual camera with looping
    fn stream_video_to_virtual_camera(
        path: &std::path::Path,
        initial_filter: FilterType,
        filter_rx: &mut tokio::sync::watch::Receiver<FilterType>,
        mut stop_rx: tokio::sync::oneshot::Receiver<()>,
        preview_tx: tokio::sync::mpsc::UnboundedSender<
            Arc<crate::backends::camera::types::CameraFrame>,
        >,
        progress_tx: tokio::sync::mpsc::UnboundedSender<(f64, f64, f64)>,
        control_rx: tokio::sync::mpsc::UnboundedReceiver<VideoPlaybackCommand>,
        initial_seek_position: f64,
        initial_paused: bool,
    ) -> Result<(), String> {
        use crate::backends::virtual_camera::{VideoDecoder, VirtualCameraManager};

        // Create video decoder
        let decoder = VideoDecoder::new(path)
            .map_err(|e| format!("Failed to create video decoder: {}", e))?;

        // Apply initial seek position if set (user sought while not streaming)
        if initial_seek_position > 0.0 {
            info!(initial_seek_position, "Applying initial seek position");
            decoder.seek(initial_seek_position);
        }

        let (width, height) = decoder.dimensions();

        // Create and start virtual camera manager
        let mut manager = VirtualCameraManager::new();
        manager.set_filter(initial_filter);
        // File sources should not be mirrored - output exactly as the file content

        if let Err(e) = manager.start(width, height) {
            return Err(format!("Failed to start virtual camera: {}", e));
        }

        info!(
            width,
            height,
            has_audio = decoder.has_audio(),
            path = %path.display(),
            "Streaming video to virtual camera (looping)"
        );

        // Get preroll frame immediately for instant preview
        if let Some(preroll) = decoder.preroll_frame() {
            let frame_arc = Arc::new(preroll);
            if let Err(e) = manager.push_frame(&frame_arc) {
                warn!(?e, "Failed to push preroll frame to virtual camera");
            }
            let _ = preview_tx.send(Arc::clone(&frame_arc));
        }

        let mut frame_count = 0u64;
        let mut last_progress_update = std::time::Instant::now();
        let mut paused = initial_paused;
        let mut control_rx = control_rx;

        // Apply initial paused state
        if initial_paused {
            decoder.set_paused(true);
            info!("Starting video in paused state");
        }

        loop {
            // Check for stop signal
            match stop_rx.try_recv() {
                Ok(()) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    info!("Stop signal received, stopping video stream");
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }

            // Check for playback control commands
            let mut needs_frame_update = false;
            while let Ok(cmd) = control_rx.try_recv() {
                match cmd {
                    VideoPlaybackCommand::Seek(position) => {
                        info!(position, "Seeking video");
                        decoder.seek(position);
                        // When seeking while paused, we need to pull a frame to update display
                        if paused {
                            needs_frame_update = true;
                        }
                    }
                    VideoPlaybackCommand::TogglePause => {
                        paused = !paused;
                        decoder.set_paused(paused);
                        info!(paused, "Video pause toggled");
                    }
                    VideoPlaybackCommand::SetPaused(p) => {
                        paused = p;
                        decoder.set_paused(paused);
                        info!(paused, "Video pause set");
                    }
                }
            }

            // Check for filter updates
            if filter_rx.has_changed().unwrap_or(false) {
                let new_filter = *filter_rx.borrow_and_update();
                manager.set_filter(new_filter);
                info!(?new_filter, "Virtual camera filter updated");
            }

            // Send progress updates at regular intervals
            use crate::constants::virtual_camera as vc_timing;
            if last_progress_update.elapsed() >= vc_timing::PROGRESS_UPDATE_INTERVAL {
                if let (Some(pos), Some(dur)) = (decoder.position(), decoder.duration()) {
                    let progress = if dur > 0.0 {
                        (pos / dur).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let _ = progress_tx.send((pos, dur, progress));
                }
                last_progress_update = std::time::Instant::now();
            }

            // If paused and no frame update needed, sleep briefly and continue
            if paused && !needs_frame_update {
                std::thread::sleep(vc_timing::PAUSE_CHECK_INTERVAL);
                continue;
            }

            // If we need a frame update while paused (after seeking), temporarily unpause to get a frame
            if needs_frame_update && paused {
                decoder.set_paused(false);
            }

            // Get next frame from video
            match decoder.next_frame() {
                Some(frame) => {
                    // Wrap frame in Arc
                    let frame_arc = Arc::new(frame);

                    // Push to virtual camera (file sources are never mirrored - they
                    // display content as-is, unlike camera selfie preview which mirrors)
                    if let Err(e) = manager.push_frame(&frame_arc) {
                        warn!(?e, "Failed to push frame to virtual camera");
                    }

                    // Send frame to preview
                    let _ = preview_tx.send(Arc::clone(&frame_arc));

                    // If we got a frame update while paused (after seeking), re-pause and send progress
                    if needs_frame_update && paused {
                        decoder.set_paused(true);
                        // Send immediate progress update
                        if let (Some(pos), Some(dur)) = (decoder.position(), decoder.duration()) {
                            let progress = if dur > 0.0 {
                                (pos / dur).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let _ = progress_tx.send((pos, dur, progress));
                        }
                    }

                    frame_count += 1;
                    if frame_count % 30 == 0 {
                        debug!(frame = frame_count, "Video streaming");
                    }
                }
                None => {
                    // If we temporarily unpaused for a frame update, re-pause even if no frame
                    if needs_frame_update && paused {
                        decoder.set_paused(true);
                    }
                    // Check if we hit end of stream
                    if decoder.is_eos() {
                        info!("Video ended, restarting for loop");
                        if let Err(e) = decoder.restart() {
                            warn!(?e, "Failed to restart video, continuing");
                        }
                    } else {
                        // No frame available, wait a bit
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
            }
        }

        decoder.stop();

        if let Err(e) = manager.stop() {
            warn!(?e, "Error stopping virtual camera");
        }

        Ok(())
    }

    fn handle_virtual_camera_started(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Virtual camera streaming started successfully");
        Task::perform(
            async {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            },
            |_| cosmic::Action::App(Message::UpdateVirtualCameraDuration),
        )
    }

    fn handle_virtual_camera_stopped(
        &mut self,
        result: Result<(), String>,
    ) -> Task<cosmic::Action<Message>> {
        self.virtual_camera = VirtualCameraState::Idle;
        // Clear the file source preview receiver (only relevant for file source streaming)
        self.file_source_preview_receiver = None;

        // For video file sources, preserve the current position and duration
        // This allows continued seeking while not streaming from where playback stopped
        let mut post_stop_task = None;
        if let Some((current_position, duration, _)) = self.video_file_progress {
            if let Some(FileSource::Video(ref path)) = self.virtual_camera_file_source {
                // Use the current playback position, not the stored seek position
                let position = current_position;
                let progress = if duration > 0.0 {
                    position / duration
                } else {
                    0.0
                };
                self.video_file_progress = Some((position, duration, progress));
                // Update the stored seek position so it's used if streaming restarts
                self.video_preview_seek_position = position;

                // If video was playing during streaming, continue playing in preview
                // Otherwise just load a static frame at the current position
                if !self.video_file_paused {
                    info!("Video was playing during streaming, continuing preview playback");
                    // Use Task::done to trigger playback in next update cycle
                    // This ensures clean state after streaming has fully stopped
                    post_stop_task = Some(Task::done(cosmic::Action::App(
                        Message::StartVideoPreviewPlayback,
                    )));
                } else {
                    // Load preview frame at the current seek position
                    let path = path.clone();
                    post_stop_task = Some(Task::perform(
                        async move {
                            use crate::backends::virtual_camera::load_video_frame_at_position;

                            match load_video_frame_at_position(&path, position) {
                                Ok(frame) => Some(Arc::new(frame)),
                                Err(e) => {
                                    warn!(?e, "Failed to load preview frame after stopping");
                                    None
                                }
                            }
                        },
                        |frame| cosmic::Action::App(Message::VideoSeekPreviewLoaded(frame)),
                    ));
                }
            } else {
                self.video_file_progress = None;
            }
        }

        // Note: We preserve video_file_paused state so it's applied when streaming restarts
        self.video_playback_control_tx = None;

        // Clear any active transition to ensure UI is enabled
        // This is important when stopping file source streaming, as no frames will arrive
        // to naturally clear the transition
        self.transition_state.clear();

        match result {
            Ok(()) => {
                info!("Virtual camera stopped successfully");
            }
            Err(err) => {
                error!(error = %err, "Virtual camera error");
            }
        }

        post_stop_task.unwrap_or_else(Task::none)
    }

    fn handle_update_virtual_camera_duration(&mut self) -> Task<cosmic::Action<Message>> {
        if self.virtual_camera.is_streaming() {
            return Task::perform(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                },
                |_| cosmic::Action::App(Message::UpdateVirtualCameraDuration),
            );
        }
        Task::none()
    }

    fn handle_open_virtual_camera_file(&self) -> Task<cosmic::Action<Message>> {
        info!("Opening file picker for virtual camera source");

        Task::perform(
            async {
                use rfd::AsyncFileDialog;

                let file = AsyncFileDialog::new()
                    .add_filter(
                        crate::fl!("virtual-camera-file-filter-name"),
                        &[
                            "png", "jpg", "jpeg", "gif", "bmp", "webp", "mp4", "mkv", "webm",
                            "avi", "mov",
                        ],
                    )
                    .pick_file()
                    .await;

                if let Some(file) = file {
                    let path = file.path().to_path_buf();
                    let extension = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_lowercase())
                        .unwrap_or_default();

                    // Determine if it's an image or video based on extension
                    use crate::constants::file_formats;
                    if file_formats::is_image_extension(&extension) {
                        Some(FileSource::Image(path))
                    } else if file_formats::is_video_extension(&extension) {
                        Some(FileSource::Video(path))
                    } else {
                        None
                    }
                } else {
                    None
                }
            },
            |file_source| cosmic::Action::App(Message::VirtualCameraFileSelected(file_source)),
        )
    }

    fn handle_virtual_camera_file_selected(
        &mut self,
        file_source: Option<FileSource>,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(ref source) = file_source {
            info!(?source, "Virtual camera file source selected");

            // Get the path to load preview from
            let path = match source {
                FileSource::Image(p) | FileSource::Video(p) => p.clone(),
            };

            let is_video = matches!(source, FileSource::Video(_));
            self.virtual_camera_file_source = file_source;

            // Reset seek position when a new file is selected
            // Start in paused state since the video isn't playing yet (just showing preview frame)
            self.video_preview_seek_position = 0.0;
            self.video_file_paused = true;

            // Load preview frame (and duration for videos) asynchronously
            return Task::perform(
                async move {
                    use crate::backends::virtual_camera::{get_video_duration, load_preview_frame};

                    let frame = match load_preview_frame(&path) {
                        Ok(frame) => Some(Arc::new(frame)),
                        Err(e) => {
                            warn!(?e, "Failed to load preview frame");
                            None
                        }
                    };

                    // For videos, also get the duration
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
        } else {
            info!("File picker cancelled");
        }
        self.virtual_camera_file_source = file_source;
        Task::none()
    }

    fn handle_file_source_preview_loaded(
        &mut self,
        frame: Option<Arc<crate::backends::camera::types::CameraFrame>>,
        duration: Option<f64>,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(frame) = frame {
            info!(
                width = frame.width,
                height = frame.height,
                ?duration,
                "File source preview loaded"
            );
            self.current_frame = Some(frame);
            self.current_frame_is_file_source = true;
        } else {
            warn!("Failed to load file source preview");
        }

        // If we have video duration, set up initial progress (position at 0 or stored seek position)
        if let Some(dur) = duration {
            let position = self.video_preview_seek_position;
            let progress = if dur > 0.0 { position / dur } else { 0.0 };
            self.video_file_progress = Some((position, dur, progress));
        }

        Task::none()
    }

    fn handle_clear_virtual_camera_file(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Clearing virtual camera file source, switching back to camera");
        // Stop any preview playback
        self.stop_video_preview_playback();
        self.virtual_camera_file_source = None;
        self.current_frame_is_file_source = false;
        self.video_file_progress = None;
        self.video_preview_seek_position = 0.0;
        self.video_file_paused = false;
        // Clear current frame so camera subscription can update it with camera feed
        self.current_frame = None;
        Task::none()
    }

    fn handle_video_file_progress(
        &mut self,
        position: f64,
        duration: f64,
        progress: f64,
    ) -> Task<cosmic::Action<Message>> {
        self.video_file_progress = Some((position, duration, progress));
        Task::none()
    }

    fn handle_video_file_seek(&mut self, position: f64) -> Task<cosmic::Action<Message>> {
        // Always store the seek position for when streaming starts
        self.video_preview_seek_position = position;

        if self.virtual_camera.is_streaming() {
            // If streaming, send seek command to the streaming decoder
            if let Some(ref tx) = self.video_playback_control_tx {
                if tx.send(VideoPlaybackCommand::Seek(position)).is_ok() {
                    info!(position, "Seeking streaming video to position");
                }
            }
            Task::none()
        } else if let Some(ref tx) = self.video_preview_control_tx {
            // If preview playback is active, send seek command to preview decoder
            if tx.send(VideoPlaybackCommand::Seek(position)).is_ok() {
                info!(position, "Seeking preview video to position");
            }
            Task::none()
        } else {
            // If not streaming and no preview playback, update progress and load a preview frame
            if let Some((_, duration, _)) = self.video_file_progress {
                let progress = if duration > 0.0 {
                    position / duration
                } else {
                    0.0
                };
                self.video_file_progress = Some((position, duration, progress));
            }

            // Load preview frame at the new position
            if let Some(FileSource::Video(ref path)) = self.virtual_camera_file_source {
                let path = path.clone();
                info!(position, "Loading preview frame at seek position");
                return Task::perform(
                    async move {
                        use crate::backends::virtual_camera::load_video_frame_at_position;

                        match load_video_frame_at_position(&path, position) {
                            Ok(frame) => Some(Arc::new(frame)),
                            Err(e) => {
                                warn!(?e, "Failed to load seek preview frame");
                                None
                            }
                        }
                    },
                    |frame| cosmic::Action::App(Message::VideoSeekPreviewLoaded(frame)),
                );
            }
            Task::none()
        }
    }

    fn handle_video_seek_preview_loaded(
        &mut self,
        frame: Option<Arc<crate::backends::camera::types::CameraFrame>>,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(frame) = frame {
            debug!(
                width = frame.width,
                height = frame.height,
                "Seek preview frame loaded"
            );
            self.current_frame = Some(frame);
            self.current_frame_is_file_source = true;
        }
        Task::none()
    }

    fn handle_toggle_video_play_pause(&mut self) -> Task<cosmic::Action<Message>> {
        // Always toggle the paused state
        self.video_file_paused = !self.video_file_paused;
        info!(paused = self.video_file_paused, "Video play/pause toggled");

        if self.virtual_camera.is_streaming() {
            // If streaming, send the command to the streaming decoder
            if let Some(ref tx) = self.video_playback_control_tx {
                let _ = tx.send(VideoPlaybackCommand::TogglePause);
            }
        } else {
            // If not streaming, start or stop preview playback
            if self.video_file_paused {
                // Stop preview playback
                self.stop_video_preview_playback();
            } else {
                // Start preview playback
                return self.start_video_preview_playback();
            }
        }
        Task::none()
    }

    fn handle_video_preview_playback_update(
        &mut self,
        frame: Arc<crate::backends::camera::types::CameraFrame>,
        position: f64,
        duration: f64,
        progress: f64,
    ) -> Task<cosmic::Action<Message>> {
        self.current_frame = Some(frame);
        self.current_frame_is_file_source = true;
        self.video_file_progress = Some((position, duration, progress));
        self.video_preview_seek_position = position;
        Task::none()
    }

    fn handle_video_preview_playback_stopped(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Video preview playback stopped");
        self.video_preview_control_tx = None;
        self.video_preview_stop_tx = None;
        Task::none()
    }

    /// Start video preview playback (when not streaming)
    fn start_video_preview_playback(&mut self) -> Task<cosmic::Action<Message>> {
        // Only start if we have a video file and are not already playing
        let path = match &self.virtual_camera_file_source {
            Some(FileSource::Video(p)) => p.clone(),
            _ => return Task::none(),
        };

        // Stop any existing preview playback
        self.stop_video_preview_playback();

        info!("Starting video preview playback");

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
        let (frame_tx, frame_rx) = tokio::sync::mpsc::unbounded_channel();

        self.video_preview_stop_tx = Some(stop_tx);
        self.video_preview_control_tx = Some(control_tx);

        let initial_position = self.video_preview_seek_position;

        // Spawn preview playback thread
        std::thread::spawn(move || {
            Self::run_video_preview_playback(path, initial_position, stop_rx, control_rx, frame_tx);
        });

        // Return a task that receives frames and sends messages
        use futures::StreamExt;
        Task::run(
            futures::stream::unfold(frame_rx, |mut rx| async move {
                rx.recv().await.map(|(frame, pos, dur, progress)| {
                    (
                        Message::VideoPreviewPlaybackUpdate(frame, pos, dur, progress),
                        rx,
                    )
                })
            })
            .chain(futures::stream::once(async {
                Message::VideoPreviewPlaybackStopped
            })),
            cosmic::Action::App,
        )
    }

    /// Stop video preview playback
    fn stop_video_preview_playback(&mut self) {
        if let Some(stop_tx) = self.video_preview_stop_tx.take() {
            let _ = stop_tx.send(());
        }
        self.video_preview_control_tx = None;
    }

    /// Run video preview playback in a background thread
    fn run_video_preview_playback(
        path: std::path::PathBuf,
        initial_position: f64,
        mut stop_rx: tokio::sync::oneshot::Receiver<()>,
        mut control_rx: tokio::sync::mpsc::UnboundedReceiver<VideoPlaybackCommand>,
        frame_tx: tokio::sync::mpsc::UnboundedSender<(
            Arc<crate::backends::camera::types::CameraFrame>,
            f64,
            f64,
            f64,
        )>,
    ) {
        use crate::backends::virtual_camera::VideoDecoder;

        let decoder = match VideoDecoder::new(&path) {
            Ok(d) => d,
            Err(e) => {
                warn!(?e, "Failed to create preview decoder");
                return;
            }
        };

        // Seek to initial position
        if initial_position > 0.0 {
            decoder.seek(initial_position);
        }

        use crate::constants::virtual_camera as vc_timing;

        let mut paused = false;
        let mut last_frame_time = std::time::Instant::now();
        let frame_duration = vc_timing::IMAGE_STREAM_FRAME_DURATION; // ~30fps

        loop {
            // Check for stop signal
            match stop_rx.try_recv() {
                Ok(()) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    break;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }

            // Check for control commands
            while let Ok(cmd) = control_rx.try_recv() {
                match cmd {
                    VideoPlaybackCommand::Seek(position) => {
                        decoder.seek(position);
                    }
                    VideoPlaybackCommand::TogglePause => {
                        paused = !paused;
                        decoder.set_paused(paused);
                    }
                    VideoPlaybackCommand::SetPaused(p) => {
                        paused = p;
                        decoder.set_paused(paused);
                    }
                }
            }

            if paused {
                std::thread::sleep(vc_timing::PAUSE_CHECK_INTERVAL);
                continue;
            }

            // Get next frame
            if let Some(frame) = decoder.next_frame() {
                let frame_arc = Arc::new(frame);

                // Get progress info
                let position = decoder.position().unwrap_or(0.0);
                let duration = decoder.duration().unwrap_or(1.0);
                let progress = if duration > 0.0 {
                    position / duration
                } else {
                    0.0
                };

                // Send frame to UI
                if frame_tx
                    .send((frame_arc, position, duration, progress))
                    .is_err()
                {
                    break; // Receiver dropped
                }

                // Rate limiting
                let elapsed = last_frame_time.elapsed();
                if elapsed < frame_duration {
                    std::thread::sleep(frame_duration - elapsed);
                }
                last_frame_time = std::time::Instant::now();
            } else if decoder.is_eos() {
                // Video ended, loop
                if decoder.restart().is_err() {
                    break;
                }
            }
        }
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

        // Update virtual camera filter if streaming
        if self.virtual_camera.is_streaming() {
            self.virtual_camera.set_filter(filter);
        }

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
        // Use NetworkManager D-Bus API - works in both native and flatpak
        Task::perform(
            crate::network_manager::connect_wifi(ssid, password, security, hidden),
            |_| cosmic::Action::App(Message::Noop),
        )
    }

    fn handle_qr_copy_text(&self, text: String) -> Task<cosmic::Action<Message>> {
        info!(
            text_length = text.len(),
            "Copying text from QR code to clipboard"
        );

        // Use iced/cosmic clipboard API - works in both native and flatpak
        cosmic::iced::clipboard::write(text).map(|_: ()| cosmic::Action::App(Message::Noop))
    }
}
