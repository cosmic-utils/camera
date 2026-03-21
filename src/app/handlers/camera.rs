// SPDX-License-Identifier: GPL-3.0-only

//! Camera control handlers
//!
//! Handles camera selection, switching, frame processing, initialization,
//! hotplug events, and mirror/virtual camera settings.

use crate::app::state::{
    AppModel, CameraMode, Message, PhotoAspectRatio, RecordingState, VirtualCameraState,
};
use crate::backends::camera::v4l2_controls;
use cosmic::Task;
use std::sync::Arc;
use tracing::{debug, error, info};

impl AppModel {
    /// Trigger haptic feedback if enabled and available.
    pub(crate) fn haptic_tap(&self) {
        if self.config.haptic_feedback {
            crate::backends::haptic::vibrate(10);
        }
    }

    // =========================================================================
    // Camera Control Handlers
    // =========================================================================

    /// Stop capture and schedule a full camera re-enumeration.
    ///
    /// Cancels the active pipeline, clears the camera list (so the subscription
    /// won't restart prematurely), and after a brief delay calls
    /// `enumerate_cameras()` which delivers its result via `CameraListChanged`.
    fn stop_and_reenumerate(&mut self) -> Task<cosmic::Action<Message>> {
        self.camera_cancel_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.camera_cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.current_frame = None;
        self.available_cameras.clear();
        self.camera_dropdown_options.clear();

        Task::perform(
            async move {
                // Give the capture thread time to drop its CameraManager
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let backend = crate::backends::camera::create_backend();
                backend.enumerate_cameras()
            },
            |cameras| cosmic::Action::App(Message::CameraListChanged(cameras)),
        )
    }

    /// Shared logic for switching to a different camera by index.
    ///
    /// Sets the cancellation flag, clears the current frame, resets zoom and
    /// aspect ratio, triggers the camera/mode switch, starts the transition
    /// animation, and re-queries exposure controls.
    ///
    /// If the target camera was added via hotplug and has no libcamera path yet,
    /// a full re-enumeration is performed first to discover the correct path.
    fn do_camera_switch(&mut self, new_index: usize) -> Task<cosmic::Action<Message>> {
        // If the target camera has no libcamera path (hotplug placeholder),
        // we need a full re-enumeration first.
        let needs_enumeration = self
            .available_cameras
            .get(new_index)
            .map(|c| c.path.is_empty())
            .unwrap_or(false);

        if needs_enumeration {
            info!("Switching to hotplugged camera — full re-enumeration required");
            self.pending_hotplug_switch = self
                .available_cameras
                .get(new_index)
                .and_then(|c| c.v4l2_path())
                .map(str::to_owned);
            return self.stop_and_reenumerate();
        }

        self.camera_cancel_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.camera_cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Capture blur state from the OLD camera before changing anything.
        // blur_frame_rotation: rotation of the camera that produced the last frame
        // blur_frame_mirror: whether the old camera's preview was mirrored
        self.blur_frame_rotation = self
            .available_cameras
            .get(self.current_camera_index)
            .map(|c| c.rotation)
            .unwrap_or_default();
        self.blur_frame_mirror = self.should_mirror_preview();

        self.current_frame = None;
        self.current_camera_index = new_index;
        self.zoom_level = 1.0;
        self.photo_aspect_ratio = crate::app::state::PhotoAspectRatio::Native;

        // If switching to a back camera with flash enabled and permission errors,
        // reset flash and show the permission popup
        let switching_to_back = self
            .available_cameras
            .get(new_index)
            .and_then(|c| c.camera_location.as_deref())
            == Some("back");
        if self.flash_enabled && switching_to_back && self.flash_hardware.has_error() {
            info!("Switching to back camera with flash permission error — disabling flash");
            self.flash_enabled = false;
            self.flash_error_popup = self.flash_hardware.permission_error.clone();
        }

        // Turn off hardware flash when switching cameras (safety measure)
        self.turn_off_flash_hardware();

        self.switch_camera_or_mode(new_index, self.mode);
        let _ = self.transition_state.start();

        // Force subscription restart in case the index didn't change
        // (e.g., hotplug removed current camera and we fall back to index 0 again)
        self.camera_stream_restart_counter = self.camera_stream_restart_counter.wrapping_add(1);

        // Re-query exposure controls for the new camera
        self.query_exposure_controls_task()
    }

    /// Build human-readable camera dropdown labels from a list of camera devices.
    fn build_camera_dropdown_labels(
        cameras: &[crate::backends::camera::types::CameraDevice],
    ) -> Vec<String> {
        cameras.iter().map(|cam| cam.name.clone()).collect()
    }

    pub(crate) fn handle_switch_camera(&mut self) -> Task<cosmic::Action<Message>> {
        self.haptic_tap();
        info!(
            current_index = self.current_camera_index,
            "Received SwitchCamera message"
        );
        if self.available_cameras.len() > 1 {
            let new_index = (self.current_camera_index + 1) % self.available_cameras.len();
            let camera_name = &self.available_cameras[new_index].name;
            info!(new_index, camera = %camera_name, "Switching to camera");

            return self.do_camera_switch(new_index);
        } else {
            info!("Only one camera available, cannot switch");
        }
        Task::none()
    }

    pub(crate) fn handle_select_camera(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index < self.available_cameras.len() {
            info!(index, "Selected camera index");

            return self.do_camera_switch(index);
        }
        Task::none()
    }

    pub(crate) fn handle_camera_frame(
        &mut self,
        frame: Arc<crate::backends::camera::types::CameraFrame>,
    ) -> Task<cosmic::Action<Message>> {
        static FRAME_MSG_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = FRAME_MSG_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if count.is_multiple_of(30) {
            debug!(
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
        if self.virtual_camera.is_streaming()
            && !self.virtual_camera.is_file_source()
            && !self.virtual_camera.send_frame(Arc::clone(&frame))
        {
            debug!("Failed to send frame to virtual camera (channel closed)");
        }

        // Recording frames are sent directly from the capture thread via
        // set_recording_sender (bypasses UI for lower latency / fewer drops).

        // Drop stale frames from the old camera/mode that were already queued
        // in the iced message queue when the switch happened — but only AFTER
        // the transition blur has ended. During the blur period we want stale
        // frames so the blurred old-camera image remains visible.
        if !self.transition_state.in_transition
            && let Some(transition_start) = self.transition_state.transition_start_time
            && frame.captured_at < transition_start
        {
            debug!("Dropping stale frame captured before camera/mode switch");
            return Task::none();
        }

        // Track whether this frame is from a file source (for mirror handling)
        let is_file_source = self.virtual_camera.is_file_source();

        // Get rotation from current camera (None for file sources)
        let frame_rotation = if is_file_source {
            crate::backends::camera::types::SensorRotation::None
        } else {
            self.available_cameras
                .get(self.current_camera_index)
                .map(|c| c.rotation)
                .unwrap_or_default()
        };

        if let Some(task) = self.transition_state.on_frame_received(frame.captured_at) {
            // First frame from the new camera — update blur state so the blurred
            // preview of the NEW camera uses the correct rotation/mirror.
            self.blur_frame_rotation = frame_rotation;
            self.blur_frame_mirror = self.should_mirror_preview();
            self.current_frame = Some(Arc::clone(&frame));
            self.current_frame_is_file_source = is_file_source;
            self.current_frame_rotation = frame_rotation;
            return task.map(cosmic::Action::App);
        }

        // During HDR+ processing, the camera stream is stopped.
        // This is a safety fallback to ignore any frames that arrive during shutdown.
        // The last frame before processing started remains displayed with blur effect.
        if self.burst_mode.stage == crate::app::state::BurstModeStage::Processing {
            return Task::none();
        }

        // Collect frames for burst mode capture
        if self.burst_mode.is_collecting_frames() {
            let collection_complete = self.burst_mode.add_frame(Arc::clone(&frame));

            debug!(
                collected = self.burst_mode.frames_captured(),
                total = self.burst_mode.target_frame_count,
                "Burst mode frame collected"
            );

            if collection_complete {
                self.current_frame = Some(frame);
                self.current_frame_is_file_source = is_file_source;
                self.current_frame_rotation = frame_rotation;
                return Task::done(cosmic::Action::App(Message::BurstModeFramesCollected));
            }
        }

        self.current_frame = Some(frame);
        self.current_frame_is_file_source = is_file_source;
        self.current_frame_rotation = frame_rotation;
        Task::none()
    }

    pub(crate) fn handle_cameras_initialized(
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

        self.camera_dropdown_options = Self::build_camera_dropdown_labels(&self.available_cameras);

        self.select_format_from_cache(self.mode);

        // Set default aspect ratio based on selected format dimensions (accounting for rotation)
        if let Some(fmt) = &self.active_format {
            let rotation = self
                .available_cameras
                .get(self.current_camera_index)
                .map(|c| c.rotation)
                .unwrap_or_default();
            self.photo_aspect_ratio =
                PhotoAspectRatio::default_for_frame_with_rotation(fmt.width, fmt.height, rotation);
        }

        self.update_mode_options();
        self.update_resolution_options();
        self.update_pixel_format_options();
        self.update_framerate_options();
        self.update_codec_options();

        info!("Camera initialization complete, preview will start");

        let mut tasks: Vec<Task<cosmic::Action<Message>>> = Vec::new();

        // Query exposure controls for the current camera
        if let Some(device_path) = self.get_v4l2_device_path() {
            let path = device_path.clone();
            let focus_path = self.get_focus_device_path();
            tasks.push(Task::perform(
                async move {
                    let controls = crate::app::exposure_picker::query_exposure_controls(
                        &path,
                        focus_path.as_deref(),
                    );
                    let settings = crate::app::exposure_picker::get_exposure_settings(
                        &path,
                        &controls,
                        focus_path.as_deref(),
                    );
                    let color_settings =
                        crate::app::exposure_picker::get_color_settings(&path, &controls);
                    (controls, settings, color_settings)
                },
                |(controls, settings, color_settings)| {
                    cosmic::Action::App(Message::ExposureControlsQueried(
                        Box::new(controls),
                        settings,
                        color_settings,
                    ))
                },
            ));
        }

        // Probe video encoders in the background. Builds a short videotestsrc
        // pipeline per encoder to detect broken ones (e.g. V4L2 encoders that
        // register but don't actually work).
        let encoder_names: Vec<String> = self
            .available_video_encoders
            .iter()
            .map(|e| e.element_name.clone())
            .collect();
        tasks.push(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::media::encoders::detection::probe_broken_encoders(&encoder_names)
                })
                .await
                .unwrap_or_default()
            },
            |broken| cosmic::Action::App(Message::BrokenEncodersDetected(broken)),
        ));

        Task::batch(tasks)
    }

    pub(crate) fn handle_broken_encoders_detected(
        &mut self,
        broken: Vec<String>,
    ) -> Task<cosmic::Action<Message>> {
        if broken.is_empty() {
            info!("All video encoders passed probe");
            return Task::none();
        }

        info!(broken = ?broken, "Removing broken video encoders");

        // Remove broken encoders from the available list
        self.available_video_encoders
            .retain(|enc| !broken.contains(&enc.element_name));

        // Rebuild the dropdown options
        self.video_encoder_dropdown_options = self
            .available_video_encoders
            .iter()
            .map(|enc| {
                enc.display_name
                    .replace(" (HW)", " (hardware accelerated)")
                    .replace(" (SW)", " (software)")
            })
            .collect();

        // Reset the selected index if it's now out of bounds
        if self.current_video_encoder_index >= self.available_video_encoders.len() {
            self.current_video_encoder_index = 0;
        }

        Task::none()
    }

    pub(crate) fn handle_camera_list_changed(
        &mut self,
        new_cameras: Vec<crate::backends::camera::types::CameraDevice>,
    ) -> Task<cosmic::Action<Message>> {
        info!(
            old_count = self.available_cameras.len(),
            new_count = new_cameras.len(),
            "Camera list changed (hotplug event)"
        );

        let old_current = self
            .available_cameras
            .get(self.current_camera_index)
            .cloned();
        let current_camera_still_available = old_current.as_ref().is_some_and(|current| {
            new_cameras
                .iter()
                .any(|c| c.path == current.path && c.name == current.name)
        });

        self.available_cameras = new_cameras;
        self.camera_dropdown_options = Self::build_camera_dropdown_labels(&self.available_cameras);

        if !current_camera_still_available {
            // Stop recording if the camera used for recording is disconnected
            if self.recording.is_recording() {
                info!("Camera disconnected during recording, stopping recording gracefully");
                if let Some(sender) = self.recording.take_stop_sender() {
                    let _ = sender.send(());
                }
                self.recording = RecordingState::Idle;
            }

            // Stop virtual camera streaming if the camera used for streaming is disconnected
            if self.virtual_camera.is_streaming() {
                info!("Camera disconnected during virtual camera streaming, stopping stream");
                if let Some(sender) = self.virtual_camera.take_stop_sender() {
                    let _ = sender.send(());
                }
                self.virtual_camera = VirtualCameraState::Idle;
            }

            if self.available_cameras.is_empty() {
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
                // If there's a pending hotplug switch, find the target camera
                // by its V4L2 device path; otherwise default to index 0.
                let target_index = self
                    .pending_hotplug_switch
                    .take()
                    .and_then(|target| {
                        self.available_cameras
                            .iter()
                            .position(|c| c.v4l2_path() == Some(target.as_str()))
                    })
                    .unwrap_or(0);

                info!(target_index, "Switching to camera after re-enumeration");
                self.current_camera_index = target_index;

                return Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        target_index
                    },
                    |index| cosmic::Action::App(Message::SelectCamera(index)),
                );
            }
        } else if let Some(current) = old_current
            && let Some(new_index) = self
                .available_cameras
                .iter()
                .position(|c| c.path == current.path && c.name == current.name)
        {
            self.current_camera_index = new_index;
        }
        Task::none()
    }

    /// Handle device nodes removed (camera unplugged).
    ///
    /// If the current camera is still present, just removes the disconnected
    /// camera from the list without interrupting the stream. Otherwise stops
    /// capture, re-enumerates, and forwards via `CameraListChanged`.
    pub(crate) fn handle_hotplug_device_removed(
        &mut self,
        removed_nodes: Vec<String>,
    ) -> Task<cosmic::Action<Message>> {
        let removed_paths: Vec<String> = removed_nodes
            .iter()
            .map(|n| format!("/dev/{}", n))
            .collect();

        // Check if the current camera's V4L2 device was removed
        let current_camera_removed = self
            .available_cameras
            .get(self.current_camera_index)
            .and_then(|c| c.v4l2_path())
            .map(|path| removed_paths.iter().any(|r| r == path))
            .unwrap_or(true); // If no device_info, assume affected

        if !current_camera_removed {
            // Current camera is fine — just remove the unplugged camera(s)
            let before = self.available_cameras.len();
            self.available_cameras.retain(|c| {
                c.v4l2_path()
                    .map(|path| !removed_paths.iter().any(|r| r == path))
                    .unwrap_or(true) // Keep cameras without device_info
            });

            info!(
                before,
                after = self.available_cameras.len(),
                "Removed unplugged camera from list without stopping stream"
            );

            self.camera_dropdown_options =
                Self::build_camera_dropdown_labels(&self.available_cameras);

            if self.current_camera_index >= self.available_cameras.len() {
                self.current_camera_index = 0;
            }

            return Task::none();
        }

        info!("Current camera unplugged — stopping capture for re-enumeration");
        self.stop_and_reenumerate()
    }

    /// Handle new device nodes added (camera plugged in).
    ///
    /// Does NOT stop the current capture stream. Instead, creates lightweight
    /// `CameraDevice` entries from V4L2 info so the switch button appears
    /// immediately. Full enumeration happens when the user actually switches.
    pub(crate) fn handle_hotplug_device_added(
        &mut self,
        new_devices: Vec<(String, String)>,
    ) -> Task<cosmic::Action<Message>> {
        if new_devices.is_empty() {
            info!("Hotplug device added but no new capture devices found");
            return Task::none();
        }

        info!(
            count = new_devices.len(),
            "Hotplug device added — adding to camera list without stopping stream"
        );

        use crate::backends::camera::types::{CameraDevice, DeviceInfo};

        for (dev_path, card_name) in &new_devices {
            if self
                .available_cameras
                .iter()
                .any(|c| c.v4l2_path() == Some(dev_path.as_str()))
            {
                debug!(dev_path, "V4L2 device already known, skipping");
                continue;
            }

            info!(dev_path, card_name, "Adding hotplugged camera");

            let card = card_name.clone();
            self.available_cameras.push(CameraDevice {
                name: card.clone(),
                device_info: Some(DeviceInfo {
                    card,
                    path: dev_path.clone(),
                    real_path: dev_path.clone(),
                    driver: String::new(),
                }),
                camera_location: Some("external".to_string()),
                ..Default::default()
            });
        }

        self.camera_dropdown_options = Self::build_camera_dropdown_labels(&self.available_cameras);

        Task::none()
    }

    pub(crate) fn handle_start_camera_transition(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Starting camera transition with blur effect");
        self.blur_frame_rotation = self.current_frame_rotation;
        self.blur_frame_mirror = self.should_mirror_preview();
        let _ = self.transition_state.start();
        Task::none()
    }

    pub(crate) fn handle_clear_transition_blur(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Clearing transition blur effect");
        self.transition_state.clear();
        Task::none()
    }

    pub(crate) fn handle_toggle_mirror_preview(&mut self) -> Task<cosmic::Action<Message>> {
        use cosmic::cosmic_config::CosmicConfigEntry;

        self.config.mirror_preview = !self.config.mirror_preview;
        info!(
            mirror_preview = self.config.mirror_preview,
            "Mirror preview toggled"
        );

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save mirror preview setting");
        }
        Task::none()
    }

    pub(crate) fn handle_toggle_haptic_feedback(&mut self) -> Task<cosmic::Action<Message>> {
        use cosmic::cosmic_config::CosmicConfigEntry;

        self.config.haptic_feedback = !self.config.haptic_feedback;
        info!(
            haptic_feedback = self.config.haptic_feedback,
            "Haptic feedback toggled"
        );

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save haptic feedback setting");
        }
        Task::none()
    }

    pub(crate) fn handle_toggle_virtual_camera_enabled(&mut self) -> Task<cosmic::Action<Message>> {
        use cosmic::cosmic_config::CosmicConfigEntry;

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

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save virtual camera setting");
        }
        Task::none()
    }

    // =========================================================================
    // Privacy Cover Detection
    // =========================================================================

    /// Handle privacy cover status change
    pub(crate) fn handle_privacy_cover_status_changed(
        &mut self,
        is_closed: bool,
    ) -> Task<cosmic::Action<Message>> {
        if self.privacy_cover_closed != is_closed {
            info!(
                privacy_cover_closed = is_closed,
                "Privacy cover status changed"
            );
            self.privacy_cover_closed = is_closed;
        }
        Task::none()
    }

    /// Check privacy cover status for the current camera
    ///
    /// Returns a task that sends PrivacyCoverStatusChanged if camera has privacy control.
    pub fn check_privacy_status(&self) -> Option<Task<cosmic::Action<Message>>> {
        // Only check if camera has privacy control
        if !self.available_exposure_controls.has_privacy {
            return None;
        }

        let device_path = self.get_v4l2_device_path()?;
        let path = device_path.clone();

        Some(Task::perform(
            async move {
                // Read the privacy control value (1 = closed/blocked, 0 = open)
                v4l2_controls::get_control(&path, v4l2_controls::V4L2_CID_PRIVACY)
                    .map(|v| v != 0)
                    .unwrap_or(false)
            },
            |is_closed| cosmic::Action::App(Message::PrivacyCoverStatusChanged(is_closed)),
        ))
    }
}
