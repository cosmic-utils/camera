// SPDX-License-Identifier: GPL-3.0-only

//! Camera control handlers
//!
//! Handles camera selection, switching, frame processing, initialization,
//! hotplug events, and mirror/virtual camera settings.

use crate::app::state::{AppModel, CameraMode, Message, PhotoAspectRatio, VirtualCameraState};
use crate::backends::camera::v4l2_controls;
use cosmic::Task;
use std::sync::Arc;
use tracing::{debug, error, info};

impl AppModel {
    // =========================================================================
    // Camera Control Handlers
    // =========================================================================

    /// Shared logic for switching to a different camera by index.
    ///
    /// Sets the cancellation flag, clears the current frame, resets zoom and
    /// aspect ratio, triggers the camera/mode switch, starts the transition
    /// animation, and re-queries exposure controls.
    fn do_camera_switch(&mut self, new_index: usize) -> Task<cosmic::Action<Message>> {
        self.camera_cancel_flag
            .store(true, std::sync::atomic::Ordering::Release);
        self.camera_cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Clear current frame to avoid accessing invalid mapped buffers
        self.current_frame = None;

        self.current_camera_index = new_index;
        self.zoom_level = 1.0;
        self.photo_aspect_ratio = crate::app::state::PhotoAspectRatio::Native;

        self.switch_camera_or_mode(new_index, self.mode);
        let _ = self.transition_state.start();

        // Re-query exposure controls for the new camera
        self.query_exposure_controls_task()
    }

    /// Build human-readable camera dropdown labels from a list of camera devices.
    ///
    /// Strips the " (V4L2)" suffix that the V4L2 backend appends to camera names.
    fn build_camera_dropdown_labels(
        cameras: &[crate::backends::camera::types::CameraDevice],
    ) -> Vec<String> {
        cameras
            .iter()
            .map(|cam| {
                cam.name
                    .strip_suffix(" (V4L2)")
                    .unwrap_or(&cam.name)
                    .to_string()
            })
            .collect()
    }

    pub(crate) fn handle_switch_camera(&mut self) -> Task<cosmic::Action<Message>> {
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

        if let Some(task) = self.transition_state.on_frame_received() {
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

        self.active_format = {
            info!("Photo mode: selecting maximum resolution");
            crate::app::format_picker::preferences::select_max_resolution_format(&formats)
        };

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

        let current_camera_still_available =
            if let Some(current) = self.available_cameras.get(self.current_camera_index) {
                new_cameras
                    .iter()
                    .any(|c| c.path == current.path && c.name == current.name)
            } else {
                false
            };

        self.available_cameras = new_cameras.clone();
        self.camera_dropdown_options = Self::build_camera_dropdown_labels(&self.available_cameras);

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
        } else if let Some(current) = self.available_cameras.get(self.current_camera_index)
            && let Some(new_index) = new_cameras
                .iter()
                .position(|c| c.path == current.path && c.name == current.name)
        {
            self.current_camera_index = new_index;
        }
        Task::none()
    }

    pub(crate) fn handle_start_camera_transition(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Starting camera transition with blur effect");
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

    pub(crate) fn handle_select_backend(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        use crate::backends::camera::types::CameraBackendType;
        use cosmic::cosmic_config::CosmicConfigEntry;

        let new_backend = match index {
            0 => CameraBackendType::Libcamera,
            _ => CameraBackendType::PipeWire,
        };

        if new_backend == self.config.backend {
            return Task::none();
        }

        info!(
            old = %self.config.backend,
            new = %new_backend,
            "Switching camera backend"
        );

        // Stop current camera streams by setting cancel flag
        self.camera_cancel_flag
            .store(true, std::sync::atomic::Ordering::Release);

        // Clear current state
        self.current_frame = None;
        self.available_cameras.clear();
        self.available_formats.clear();
        self.active_format = None;
        self.camera_dropdown_options.clear();

        // Update backend in config
        self.config.backend = new_backend;

        // Recreate backend manager with new backend
        self.backend_manager = Some(crate::backends::camera::CameraBackendManager::new(
            new_backend,
        ));

        // Reset cancel flag for new streams
        self.camera_cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Increment restart counter to force subscription restart
        self.camera_stream_restart_counter += 1;

        // Save config
        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save backend setting");
        }

        // Re-enumerate cameras asynchronously with new backend
        let backend_type = new_backend;
        let last_camera_path = self.config.last_camera_path.clone();
        Task::perform(
            async move {
                let backend = crate::backends::camera::get_backend_for_type(backend_type);
                let cameras = backend.enumerate_cameras();
                info!(count = cameras.len(), backend = %backend_type, "Re-enumerated cameras for new backend");

                let camera_index = if let Some(ref last_path) = last_camera_path {
                    cameras
                        .iter()
                        .position(|cam| &cam.path == last_path)
                        .unwrap_or(0)
                } else {
                    0
                };

                let formats = if let Some(camera) = cameras.get(camera_index) {
                    if !camera.path.is_empty() {
                        backend.get_formats(camera, false)
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };

                (cameras, camera_index, formats)
            },
            |(cameras, index, formats)| {
                cosmic::Action::App(Message::CamerasInitialized(cameras, index, formats))
            },
        )
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
