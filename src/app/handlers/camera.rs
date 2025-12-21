// SPDX-License-Identifier: GPL-3.0-only

//! Camera control handlers
//!
//! Handles camera selection, switching, frame processing, initialization,
//! hotplug events, and mirror/virtual camera settings.

use crate::app::state::{AppModel, CameraMode, Message, PhotoAspectRatio, VirtualCameraState};
use crate::backends::camera::v4l2_controls;
use cosmic::Task;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

impl AppModel {
    // =========================================================================
    // Camera Control Handlers
    // =========================================================================

    pub(crate) fn handle_switch_camera(&mut self) -> Task<cosmic::Action<Message>> {
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

            // Reset zoom and aspect ratio when switching cameras
            self.zoom_level = 1.0;
            self.photo_aspect_ratio = crate::app::state::PhotoAspectRatio::Native;
            // Exit 3D preview when switching cameras
            self.preview_3d.enabled = false;
            self.preview_3d.rendered_preview = None;
            // Switch out of Scene mode when changing cameras (depth may not be available)
            if self.mode == CameraMode::Scene {
                self.mode = CameraMode::Photo;
            }

            self.switch_camera_or_mode(self.current_camera_index, self.mode);
            let _ = self.transition_state.start();

            // Update Kinect state after camera switch
            let kinect_task = self.update_kinect_state();

            // Re-query exposure controls for the new camera
            let exposure_task = self.query_exposure_controls_task();

            return Task::batch([kinect_task, exposure_task]);
        } else {
            info!("Only one camera available, cannot switch");
        }
        Task::none()
    }

    pub(crate) fn handle_select_camera(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        if index < self.available_cameras.len() {
            info!(index, "Selected camera index");

            let _ = self.transition_state.start();
            self.camera_cancel_flag
                .store(true, std::sync::atomic::Ordering::Release);
            self.camera_cancel_flag =
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            self.current_camera_index = index;
            self.zoom_level = 1.0; // Reset zoom when switching cameras
            // Reset aspect ratio to native when switching cameras
            self.photo_aspect_ratio = crate::app::state::PhotoAspectRatio::Native;
            // Exit 3D preview when switching cameras
            self.preview_3d.enabled = false;
            self.preview_3d.rendered_preview = None;
            // Switch out of Scene mode when changing cameras (depth may not be available)
            if self.mode == CameraMode::Scene {
                self.mode = CameraMode::Photo;
            }
            self.switch_camera_or_mode(index, self.mode);

            // Update Kinect state after camera switch (may return async task)
            let kinect_task = self.update_kinect_state();

            // Re-query exposure controls for the new camera
            let exposure_task = self.query_exposure_controls_task();

            return Task::batch([kinect_task, exposure_task]);
        }
        Task::none()
    }

    pub(crate) fn handle_camera_frame(
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
            self.current_frame = Some(Arc::clone(&frame));
            self.current_frame_is_file_source = is_file_source;
            return task.map(cosmic::Action::App);
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
                return Task::done(cosmic::Action::App(Message::BurstModeFramesCollected));
            }
        }

        // Store depth data from depth frames for use in 3D preview
        // Use actual depth dimensions, not frame dimensions (which may be RGB resolution)
        if let Some(depth_data) = &frame.depth_data {
            self.preview_3d.latest_depth_data = Some((
                frame.depth_width,
                frame.depth_height,
                Arc::clone(depth_data),
            ));
        }

        self.current_frame = Some(frame.clone());
        self.current_frame_is_file_source = is_file_source;

        // Trigger point cloud render if 3D preview mode is active
        // Use depth data from current frame or stored latest depth data
        // For Kinect with different depth/color frame rates, only render when video frame is new
        // This prevents rendering at 30fps when video is only 10fps (high-res mode)
        if self.preview_3d.enabled && self.preview_3d.latest_depth_data.is_some() {
            // Check if this is a new video frame using timestamp
            // If no timestamp (non-Kinect sources), always render
            let video_is_new = match frame.video_timestamp {
                Some(ts) => {
                    let is_new = self.preview_3d.last_render_video_timestamp != Some(ts);
                    if is_new {
                        self.preview_3d.last_render_video_timestamp = Some(ts);
                    }
                    is_new
                }
                None => true, // No timestamp means always render (non-Kinect sources)
            };

            if video_is_new {
                return self.handle_request_point_cloud_render();
            }
        }

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

        // Set default aspect ratio based on selected format dimensions
        if let Some(fmt) = &self.active_format {
            self.photo_aspect_ratio = PhotoAspectRatio::default_for_frame(fmt.width, fmt.height);
        }

        self.update_mode_options();
        self.update_resolution_options();
        self.update_pixel_format_options();
        self.update_framerate_options();
        self.update_codec_options();

        // Update Kinect state if this is a Kinect device (may return async task)
        let kinect_task = self.update_kinect_state();

        info!("Camera initialization complete, preview will start");

        // Query exposure controls for the current camera
        let exposure_task = if let Some(device_path) = self.get_v4l2_device_path() {
            let path = device_path.clone();
            Task::perform(
                async move {
                    let controls = crate::app::exposure_picker::query_exposure_controls(&path);
                    let settings =
                        crate::app::exposure_picker::get_exposure_settings(&path, &controls);
                    let color_settings =
                        crate::app::exposure_picker::get_color_settings(&path, &controls);
                    (controls, settings, color_settings)
                },
                |(controls, settings, color_settings)| {
                    cosmic::Action::App(Message::ExposureControlsQueried(
                        controls,
                        settings,
                        color_settings,
                    ))
                },
            )
        } else {
            Task::none()
        };

        Task::batch([kinect_task, exposure_task])
    }

    pub(crate) fn handle_camera_list_changed(
        &mut self,
        new_cameras: Vec<crate::backends::camera::types::CameraDevice>,
    ) -> Task<cosmic::Action<Message>> {
        use crate::backends::camera::is_depth_camera;

        info!(
            old_count = self.available_cameras.len(),
            new_count = new_cameras.len(),
            "Camera list changed (hotplug event)"
        );

        // Check if current camera is still available
        // Special handling for Kinect: if native streaming is active, consider it available
        // even if the path changed (V4L2 -> freedepth transition)
        let current_camera_still_available =
            if let Some(current) = self.available_cameras.get(self.current_camera_index) {
                // Check by path and name
                let exact_match = new_cameras
                    .iter()
                    .any(|c| c.path == current.path && c.name == current.name);

                // If native Kinect streaming is active and the current camera is a Kinect,
                // check if there's any Kinect in the new list (path may have changed)
                let kinect_still_available = self.kinect.streaming
                    && is_depth_camera(current)
                    && new_cameras.iter().any(|c| is_depth_camera(c));

                exact_match || kinect_still_available
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

        // If native Kinect streaming is active, update current_camera_index to point to the Kinect
        if self.kinect.streaming {
            if let Some(kinect_index) = new_cameras.iter().position(|c| is_depth_camera(c)) {
                self.current_camera_index = kinect_index;
            }
        }

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

        // Update Kinect state after hotplug - device might have been reconnected
        // with new device_info or the Kinect might have been disconnected
        self.update_kinect_state()
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

        if let Some(handler) = self.config_handler.as_ref() {
            if let Err(err) = self.config.write_entry(handler) {
                error!(?err, "Failed to save mirror preview setting");
            }
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

        if let Some(handler) = self.config_handler.as_ref() {
            if let Err(err) = self.config.write_entry(handler) {
                error!(?err, "Failed to save virtual camera setting");
            }
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

    // =========================================================================
    // 3D Point Cloud Preview
    // =========================================================================

    /// Handle request to render point cloud from current frame
    pub(crate) fn handle_request_point_cloud_render(&self) -> Task<cosmic::Action<Message>> {
        // Only render if 3D preview is enabled
        if !self.preview_3d.enabled {
            return Task::none();
        }

        let Some(frame) = &self.current_frame else {
            debug!("No frame available for point cloud render");
            return Task::none();
        };

        // Get depth data - prefer current frame's depth data, fall back to stored latest
        // Use actual depth dimensions, not RGB frame dimensions (which may differ for high-res modes)
        let (depth_width, depth_height, depth_data) = if let Some(depth_data) = &frame.depth_data {
            // Validate depth dimensions - if they're 0, use stored latest instead
            if frame.depth_width > 0 && frame.depth_height > 0 {
                (
                    frame.depth_width,
                    frame.depth_height,
                    Arc::clone(depth_data),
                )
            } else if let Some((w, h, _)) = &self.preview_3d.latest_depth_data {
                // Use stored dimensions but current depth data
                (*w, *h, Arc::clone(depth_data))
            } else {
                // Fall back to Kinect defaults
                (640, 480, Arc::clone(depth_data))
            }
        } else if let Some((w, h, data)) = &self.preview_3d.latest_depth_data {
            (*w, *h, Arc::clone(data))
        } else {
            debug!("No depth data available for point cloud render");
            return Task::none();
        };

        // Final validation
        if depth_width == 0 || depth_height == 0 {
            warn!("Invalid depth dimensions: {}x{}", depth_width, depth_height);
            return Task::none();
        }

        // Clone RGB data for async task
        let rgb_data = frame.data.clone();
        let rgb_width = frame.width;
        let rgb_height = frame.height;
        let (pitch, yaw) = self.preview_3d.rotation;
        let zoom = self.preview_3d.zoom;

        info!(
            rgb_width,
            rgb_height,
            rgb_bytes = rgb_data.len(),
            depth_width,
            depth_height,
            depth_pixels = depth_data.len(),
            frame_format = ?frame.format,
            "Rendering point cloud"
        );

        // Check for resolution mismatch - if RGB and depth don't match,
        // we need to use the depth dimensions and sample RGB accordingly
        if rgb_width != depth_width || rgb_height != depth_height {
            info!(
                rgb = format!("{}x{}", rgb_width, rgb_height),
                depth = format!("{}x{}", depth_width, depth_height),
                "RGB/depth resolution mismatch - using depth dimensions"
            );
        }

        // Use depth dimensions as the input base (640x480 for Kinect)
        // The shader will sample RGB at scaled coordinates if needed
        let input_width = depth_width;
        let input_height = depth_height;

        // Render at higher resolution so 3D preview fills the available space
        // Use the larger of RGB resolution or a reasonable minimum (1280x960)
        // This prevents the preview from looking small/pixelated when scaled up
        let output_width = rgb_width.max(1280);
        let output_height = rgb_height.max(960);

        // Determine depth format based on source
        // - Native Kinect backend provides depth in millimeters
        // - V4L2 Y10B pipeline provides 10-bit disparity shifted to 16-bit
        // Check if depth came from current frame (native Kinect) or from frame.depth_data
        let depth_format = if self.kinect.is_device && frame.depth_data.is_some() {
            // Native Kinect backend - depth is in millimeters
            crate::shaders::DepthFormat::Millimeters
        } else {
            // V4L2 Y10B pipeline - depth is 10-bit disparity shifted to 16-bit
            crate::shaders::DepthFormat::Disparity16
        };

        // Get mirror setting
        let mirror = self.config.mirror_preview;

        // Determine if we need to apply RGB-depth stereo registration
        // Only apply when color data is from the RGB camera (not IR or depth tonemap)
        // - If sensor_type is Ir or Depth: no registration (same sensor as depth)
        // - If depth overlay is enabled: no registration (showing depth colormap, not RGB)
        // - Only RGB sensor type WITHOUT depth overlay needs registration
        // Registration tables are built for 640x480 RGB but can be scaled to higher resolutions
        use crate::backends::camera::SensorType;
        let sensor_type = self
            .active_format
            .as_ref()
            .map(|f| f.sensor_type)
            .unwrap_or(SensorType::Rgb);
        let showing_depth_colormap = self.depth_viz.overlay_enabled;
        let apply_rgb_registration = sensor_type == SensorType::Rgb && !showing_depth_colormap;

        // Get scene view mode (point cloud vs mesh)
        let scene_view_mode = self.preview_3d.view_mode;

        info!(
            depth_format = ?depth_format,
            mirror,
            apply_rgb_registration,
            rgb_dim = format!("{}x{}", rgb_width, rgb_height),
            depth_dim = format!("{}x{}", depth_width, depth_height),
            ?sensor_type,
            showing_depth_colormap,
            ?scene_view_mode,
            "Rendering 3D scene"
        );

        Task::perform(
            async move {
                use crate::app::state::SceneViewMode;
                let result = match scene_view_mode {
                    SceneViewMode::PointCloud => crate::shaders::render_point_cloud(
                        &rgb_data,
                        &depth_data,
                        rgb_width,
                        rgb_height,
                        input_width,
                        input_height,
                        output_width,
                        output_height,
                        pitch,
                        yaw,
                        zoom,
                        depth_format,
                        mirror,
                        apply_rgb_registration,
                    )
                    .await
                    .map(|r| (r.width, r.height, r.rgba)),
                    SceneViewMode::Mesh => {
                        // Fixed depth discontinuity threshold of 0.1 meters
                        const DEPTH_DISCONTINUITY_THRESHOLD: f32 = 0.1;
                        crate::shaders::render_mesh(
                            &rgb_data,
                            &depth_data,
                            rgb_width,
                            rgb_height,
                            input_width,
                            input_height,
                            output_width,
                            output_height,
                            pitch,
                            yaw,
                            zoom,
                            depth_format,
                            mirror,
                            apply_rgb_registration,
                            DEPTH_DISCONTINUITY_THRESHOLD,
                        )
                        .await
                        .map(|r| (r.width, r.height, r.rgba))
                    }
                };

                match result {
                    Ok((width, height, rgba)) => Some((width, height, Arc::new(rgba))),
                    Err(e) => {
                        error!("Failed to render 3D scene: {}", e);
                        None
                    }
                }
            },
            |result| {
                if let Some((width, height, data)) = result {
                    cosmic::Action::App(Message::PointCloudRendered(width, height, data))
                } else {
                    cosmic::Action::App(Message::Noop)
                }
            },
        )
    }
}
