// SPDX-License-Identifier: GPL-3.0-only

//! Message update handling
//!
//! This module handles all application messages by routing them to focused handler methods.
//! The main `update()` function acts as a dispatcher, while specific handlers are implemented
//! in the `handlers` submodules organized by functional domain.
//!
//! # Handler Modules
//!
//! - `handlers::ui`: UI navigation, pickers, theatre mode, tools menu
//! - `handlers::exposure`: Exposure and camera control settings
//! - `handlers::color`: Color adjustment controls
//! - `handlers::camera`: Camera selection, frame handling, transitions
//! - `handlers::format`: Resolution, framerate, codec selection
//! - `handlers::capture`: Photo capture, video recording, zoom
//! - `handlers::virtual_camera`: Virtual camera streaming
//! - `handlers::system`: Gallery, filters, settings, recovery, QR codes

use crate::app::state::{AppModel, Message};
use cosmic::Task;
use tracing::{debug, info, warn};

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
            Message::ToggleDeviceInfo => self.handle_toggle_device_info(),

            // ===== Tools Menu =====
            Message::ToggleToolsMenu => self.handle_toggle_tools_menu(),
            Message::CloseToolsMenu => self.handle_close_tools_menu(),

            // ===== Motor/PTZ Controls =====
            Message::ToggleMotorPicker => {
                self.motor_picker_visible = !self.motor_picker_visible;
                // Close other pickers when opening motor picker
                if self.motor_picker_visible {
                    self.exposure_picker_visible = false;
                    self.color_picker_visible = false;
                    self.tools_menu_visible = false;

                    // Get initial tilt from motor control if available
                    #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
                    if self.kinect.is_device {
                        use crate::backends::camera::motor_control::{
                            get_motor_tilt, is_motor_available,
                        };
                        if is_motor_available() {
                            match get_motor_tilt() {
                                Ok(tilt) => {
                                    self.kinect.tilt_angle = tilt;
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to get Kinect tilt: {}", e);
                                }
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::CloseMotorPicker => {
                self.motor_picker_visible = false;
                Task::none()
            }
            Message::SetPanAbsolute(value) => {
                self.set_v4l2_pan(value);
                Task::none()
            }
            Message::SetTiltAbsolute(value) => {
                self.set_v4l2_tilt(value);
                Task::none()
            }
            Message::SetZoomAbsolute(value) => {
                self.set_v4l2_zoom(value);
                Task::none()
            }
            Message::ResetPanTilt => {
                self.reset_pan_tilt();
                // Also reset Kinect tilt if it's a Kinect device
                if self.kinect.is_device {
                    return self.handle_set_kinect_tilt(0);
                }
                Task::none()
            }

            // ===== Exposure Controls =====
            Message::ToggleExposurePicker => self.handle_toggle_exposure_picker(),
            Message::CloseExposurePicker => self.handle_close_exposure_picker(),
            Message::SetExposureMode(mode) => self.handle_set_exposure_mode(mode),
            Message::SetExposureCompensation(value) => self.handle_set_exposure_compensation(value),
            Message::ResetExposureCompensation => self.handle_reset_exposure_compensation(),
            Message::SetExposureTime(value) => self.handle_set_exposure_time(value),
            Message::SetGain(value) => self.handle_set_gain(value),
            Message::SetIsoSensitivity(value) => self.handle_set_iso_sensitivity(value),
            Message::SetMeteringMode(mode) => self.handle_set_metering_mode(mode),
            Message::ToggleAutoExposurePriority => self.handle_toggle_auto_exposure_priority(),
            Message::ExposureControlsQueried(controls, settings, color_settings) => {
                self.handle_exposure_controls_queried(controls, settings, color_settings)
            }
            Message::ExposureControlApplied => Task::none(),
            Message::WhiteBalanceToggled(temp) => {
                if let Some(temp_value) = temp {
                    if let Some(ref mut settings) = self.color_settings {
                        settings.white_balance_temperature = Some(temp_value);
                    }
                    info!(
                        temperature = temp_value,
                        "White balance switched to manual, preserved auto temperature"
                    );
                }
                Task::none()
            }
            Message::ExposureControlFailed(error) => {
                warn!(error = %error, "Exposure control failed");
                Task::none()
            }
            Message::ExposureBaseTimeCaptured(base_time) => {
                info!(base_time, "Captured base exposure time for EV slider");
                self.base_exposure_time = Some(base_time);
                Task::none()
            }
            Message::SetBacklightCompensation(value) => {
                self.handle_set_backlight_compensation(value)
            }
            Message::ResetExposureSettings => self.handle_reset_exposure_settings(),
            Message::ExposureModeSelected(entity) => self.handle_exposure_mode_selected(entity),

            // ===== Color Controls =====
            Message::ToggleColorPicker => self.handle_toggle_color_picker(),
            Message::CloseColorPicker => self.handle_close_color_picker(),
            Message::SetContrast(value) => self.handle_set_contrast(value),
            Message::SetSaturation(value) => self.handle_set_saturation(value),
            Message::SetSharpness(value) => self.handle_set_sharpness(value),
            Message::SetHue(value) => self.handle_set_hue(value),
            Message::ToggleAutoWhiteBalance => self.handle_toggle_auto_white_balance(),
            Message::SetWhiteBalanceTemperature(value) => {
                self.handle_set_white_balance_temperature(value)
            }
            Message::ResetColorSettings => self.handle_reset_color_settings(),

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
            Message::ToggleBurstMode => self.handle_toggle_burst_mode(),
            Message::SetBurstModeFrameCount(index) => self.handle_set_burst_mode_frame_count(index),
            Message::BurstModeProgress(progress) => self.handle_burst_mode_progress(progress),
            Message::BurstModeFramesCollected => self.handle_burst_mode_frames_collected(),
            Message::BurstModeComplete(result) => self.handle_burst_mode_complete(result),
            Message::PollBurstModeProgress => self.handle_poll_burst_mode_progress(),
            Message::ResetBurstModeState => {
                self.burst_mode.reset();
                // Ensure flash is turned off when burst mode resets (safety measure)
                self.flash_active = false;
                debug!("Burst mode state reset");
                Task::none()
            }
            Message::CyclePhotoAspectRatio => self.handle_cycle_photo_aspect_ratio(),
            Message::FlashComplete => self.handle_flash_complete(),
            Message::CyclePhotoTimer => self.handle_cycle_photo_timer(),
            Message::PhotoTimerTick => self.handle_photo_timer_tick(),
            Message::PhotoTimerAnimationFrame => Task::none(),
            Message::AbortPhotoTimer => self.handle_abort_photo_timer(),
            Message::ZoomIn => self.handle_zoom_in(),
            Message::ZoomOut => self.handle_zoom_out(),
            Message::ResetZoom => self.handle_reset_zoom(),
            Message::PhotoSaved(result) => self.handle_photo_saved(result),
            Message::SceneSaved(result) => self.handle_scene_saved(result),
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
            Message::SetAppTheme(index) => self.handle_set_app_theme(index),
            Message::SelectAudioDevice(index) => self.handle_select_audio_device(index),
            Message::SelectVideoEncoder(index) => self.handle_select_video_encoder(index),
            Message::SelectPhotoOutputFormat(index) => {
                self.handle_select_photo_output_format(index)
            }
            Message::ToggleSaveBurstRaw => self.handle_toggle_save_burst_raw(),

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

            // ===== Privacy Cover Detection =====
            Message::PrivacyCoverStatusChanged(is_closed) => {
                self.handle_privacy_cover_status_changed(is_closed)
            }

            // ===== Depth Visualization =====
            Message::ToggleDepthOverlay => self.handle_toggle_depth_overlay(),
            Message::ToggleDepthGrayscale => self.handle_toggle_depth_grayscale(),

            // ===== Calibration =====
            Message::ShowCalibrationDialog => self.handle_show_calibration_dialog(),
            Message::CloseCalibrationDialog => self.handle_close_calibration_dialog(),
            Message::StartCalibration => self.handle_start_calibration(),

            // ===== 3D Preview =====
            Message::Toggle3DPreview => self.handle_toggle_3d_preview(),
            Message::ToggleSceneViewMode => self.handle_toggle_scene_view_mode(),
            Message::Preview3DMousePressed(x, y) => self.handle_preview_3d_mouse_pressed(x, y),
            Message::Preview3DMouseMoved(x, y) => self.handle_preview_3d_mouse_moved(x, y),
            Message::Preview3DMouseReleased => self.handle_preview_3d_mouse_released(),
            Message::Reset3DPreviewRotation => self.handle_reset_3d_preview_rotation(),
            Message::Zoom3DPreview(delta) => self.handle_zoom_3d_preview(delta),
            Message::PointCloudRendered(width, height, data) => {
                self.preview_3d.rendered_preview = Some((width, height, data));
                Task::none()
            }
            Message::SecondaryDepthFrame(width, height, depth_data) => {
                // Store depth data (unused - secondary capture was never implemented)
                self.preview_3d.latest_depth_data = Some((width, height, depth_data));
                info!(width, height, "Received depth frame");
                self.handle_request_point_cloud_render()
            }
            Message::RequestPointCloudRender => self.handle_request_point_cloud_render(),

            // ===== Kinect Controls =====
            Message::SetKinectTilt(degrees) => self.handle_set_kinect_tilt(degrees),
            Message::KinectStateUpdated(_tilt) => {
                // Tilt is managed as "desired state" in UI to avoid flickering from motor feedback
                // LED is automatically managed by freedepth
                Task::none()
            }
            Message::KinectControlFailed(error) => {
                tracing::warn!(error = %error, "Kinect control failed");
                Task::none()
            }
            Message::KinectInitialized(result) => {
                match result {
                    Ok(tilt) => {
                        tracing::info!(tilt, "Kinect controller initialized");
                        self.kinect.tilt_angle = tilt;
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "Failed to initialize Kinect controller");
                    }
                }
                Task::none()
            }

            // ===== Native Kinect Streaming =====
            Message::StartNativeKinectStreaming => self.handle_start_native_kinect_streaming(),
            Message::StopNativeKinectStreaming => self.handle_stop_native_kinect_streaming(),

            Message::NativeKinectStreamingStarted => {
                info!("Native Kinect streaming started");
                self.kinect.streaming = true;
                // Start polling for frames
                Task::done(cosmic::Action::App(Message::PollNativeKinectFrames))
            }

            Message::NativeKinectStreamingFailed(error) => {
                warn!(error = %error, "Native Kinect streaming failed");
                self.kinect.streaming = false;
                self.preview_3d.enabled = false;
                Task::none()
            }

            Message::PollNativeKinectFrames => self.handle_poll_native_kinect_frames(),

            Message::Noop => Task::none(),
        }
    }
}
