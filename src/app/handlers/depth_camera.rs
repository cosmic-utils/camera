// SPDX-License-Identifier: GPL-3.0-only

//! Depth camera control handlers
//!
//! Handles depth camera device control including motor tilt, native streaming,
//! 3D preview frame polling, depth visualization settings, and calibration dialogs.
//! LED is automatically managed by freedepth based on device state.

use crate::app::state::{AppModel, Message, SceneViewMode};
use crate::backends::camera::{NativeDepthBackend, depth_device_index, rgb_to_rgba};
use cosmic::Task;
use freedepth::{DepthRegistration, TILT_MAX_DEGREES, TILT_MIN_DEGREES};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Set up GPU shader registration data from device calibration
///
/// This spawns an async task to update the point cloud and mesh processors
/// with the device-specific registration tables. Call this after the native
/// depth backend is initialized.
///
/// Uses the generic DepthRegistration trait for device-agnostic access.
pub fn setup_shader_registration_data(registration: &dyn DepthRegistration) {
    let reg_data = crate::shaders::RegistrationData {
        registration_table: registration.registration_table_flat().to_vec(),
        depth_to_rgb_shift: registration.depth_to_rgb_shift_table().to_vec(),
        target_offset: registration.target_offset(),
    };

    tokio::spawn(async move {
        if let Err(e) = crate::shaders::set_point_cloud_registration_data(&reg_data).await {
            tracing::warn!("Failed to set point cloud registration data: {}", e);
        } else {
            tracing::info!("Point cloud registration data set from device calibration");
        }
        if let Err(e) = crate::shaders::set_mesh_registration_data(&reg_data).await {
            tracing::warn!("Failed to set mesh registration data: {}", e);
        } else {
            tracing::info!("Mesh registration data set from device calibration");
        }
    });
}

impl AppModel {
    /// Handle setting depth camera tilt angle (desired state)
    ///
    /// Updates the desired tilt angle immediately in the UI, then sends the command
    /// to the motor via the global motor control interface.
    pub(crate) fn handle_set_kinect_tilt(&mut self, degrees: i8) -> Task<cosmic::Action<Message>> {
        if !self.kinect.is_device {
            return Task::none();
        }

        // Clamp to valid range (from freedepth)
        let degrees = degrees.clamp(TILT_MIN_DEGREES, TILT_MAX_DEGREES);

        // Update desired state immediately for responsive UI
        self.kinect.tilt_angle = degrees;

        // Send command to motor via global motor control
        use crate::backends::camera::depth_controller::set_motor_tilt;
        if let Err(e) = set_motor_tilt(degrees) {
            tracing::warn!("Failed to set depth camera tilt: {}", e);
        }

        Task::none()
    }

    /// Update depth camera state when camera changes
    ///
    /// Call this after switching cameras to update is_depth_camera flag.
    /// Note: freedepth initialization is deferred until motor picker is opened
    /// to avoid conflicts with V4L2 streaming.
    pub(crate) fn update_kinect_state(&mut self) -> Task<cosmic::Action<Message>> {
        use crate::backends::camera::is_depth_camera;

        let was_depth_camera = self.kinect.is_device;

        if let Some(ref camera) = self.available_cameras.get(self.current_camera_index) {
            // Debug: log device info for depth camera detection
            debug!(
                camera_name = %camera.name,
                camera_path = %camera.path,
                has_device_info = camera.device_info.is_some(),
                driver = camera.device_info.as_ref().map(|i| i.driver.as_str()).unwrap_or("none"),
                "Checking camera for depth camera"
            );

            self.kinect.is_device = is_depth_camera(camera);

            info!(
                camera_name = %camera.name,
                was_depth_camera = was_depth_camera,
                is_depth_camera = self.kinect.is_device,
                camera_index = self.current_camera_index,
                "Updated depth camera state"
            );

            if self.kinect.is_device {
                info!("Depth camera detected - motor controls available");
            } else if was_depth_camera {
                info!("Switching away from depth camera device");
            }
        } else {
            info!(
                was_depth_camera = was_depth_camera,
                camera_index = self.current_camera_index,
                num_cameras = self.available_cameras.len(),
                "No camera at index - setting is_depth_camera to false"
            );
            self.kinect.is_device = false;
        }
        Task::none()
    }

    /// Check if the current camera is a depth sensor (not RGB)
    pub(crate) fn is_current_camera_depth_sensor(&self) -> bool {
        if let Some(camera) = self.available_cameras.get(self.current_camera_index) {
            let name_lower = camera.name.to_lowercase();
            let path_lower = camera.path.to_lowercase();
            name_lower.contains("depth") || path_lower.contains("depth")
        } else {
            false
        }
    }

    /// Handle starting native depth camera streaming for simultaneous RGB+depth
    pub(crate) fn handle_start_native_kinect_streaming(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Starting native depth camera streaming for simultaneous RGB+depth");

        // Get device index from current camera
        let device_index = self
            .available_cameras
            .get(self.current_camera_index)
            .and_then(|d| depth_device_index(&d.path))
            .unwrap_or(0);

        // Create and start the native backend with default format
        let mut backend = NativeDepthBackend::new();
        match backend.start(device_index) {
            Ok(()) => {
                // Set up registration data for depth-to-RGB alignment
                if let Some(registration) = backend.get_registration() {
                    // Store calibration info for UI display (device-agnostic)
                    self.kinect.calibration_info = Some(registration.registration_summary());

                    // Store registration data for scene capture
                    self.kinect.registration_data =
                        Some(crate::pipelines::scene::RegistrationData {
                            registration_table: registration.registration_table_flat().to_vec(),
                            depth_to_rgb_shift: registration.depth_to_rgb_shift_table().to_vec(),
                            target_offset: registration.target_offset(),
                            reg_x_val_scale: 256,
                            reg_scale_x: 1.0,
                            reg_scale_y: 1.0,
                            reg_y_offset: 0,
                        });

                    // Set up GPU shader registration data
                    setup_shader_registration_data(registration);
                } else {
                    warn!("No registration data available from depth camera backend");
                    self.kinect.calibration_info = None;
                    self.kinect.registration_data = None;
                }

                self.kinect.native_backend = Some(backend);
                self.kinect.streaming = true;
                info!("Native depth camera streaming started successfully");

                // Start polling for frames
                Task::done(cosmic::Action::App(Message::PollNativeKinectFrames))
            }
            Err(e) => {
                warn!(error = %e, "Failed to start native depth camera streaming");
                self.preview_3d.enabled = false;
                Task::none()
            }
        }
    }

    /// Handle stopping native depth camera streaming
    pub(crate) fn handle_stop_native_kinect_streaming(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Stopping native depth camera streaming");
        if let Some(mut backend) = self.kinect.native_backend.take() {
            backend.stop();
        }
        self.kinect.streaming = false;
        self.preview_3d.rendered_preview = None;
        self.kinect.calibration_info = None;
        self.kinect.registration_data = None;
        Task::none()
    }

    /// Handle polling for native depth camera frames
    pub(crate) fn handle_poll_native_kinect_frames(&mut self) -> Task<cosmic::Action<Message>> {
        if !self.kinect.streaming {
            return Task::none();
        }

        let mut tasks = Vec::new();

        if let Some(ref backend) = self.kinect.native_backend {
            let mut video_frame_is_new = false;

            if let Some(video_frame) = backend.get_video_frame() {
                video_frame_is_new =
                    self.preview_3d.last_render_video_timestamp != Some(video_frame.timestamp);

                let rgba_data = rgb_to_rgba(&video_frame.rgb_data);

                use crate::backends::camera::types::{CameraFrame, PixelFormat};
                let frame = CameraFrame {
                    width: video_frame.width,
                    height: video_frame.height,
                    stride: video_frame.width * 4,
                    data: rgba_data.into(),
                    format: PixelFormat::RGBA,
                    captured_at: std::time::Instant::now(),
                    depth_data: None,
                    depth_width: 0,
                    depth_height: 0,
                    video_timestamp: Some(video_frame.timestamp),
                };

                self.current_frame = Some(std::sync::Arc::new(frame));
                self.current_frame_is_file_source = false;

                if let Some(task) = self.transition_state.on_frame_received() {
                    tasks.push(task.map(cosmic::Action::App));
                }

                if video_frame_is_new {
                    self.preview_3d.last_render_video_timestamp = Some(video_frame.timestamp);
                }
            }

            if let Some(depth_frame) = backend.get_depth_frame() {
                self.preview_3d.latest_depth_data = Some((
                    depth_frame.width,
                    depth_frame.height,
                    std::sync::Arc::from(depth_frame.depth_mm.as_slice()),
                ));

                if self.preview_3d.enabled && video_frame_is_new {
                    tasks.push(self.handle_request_point_cloud_render());
                }
            }
        }

        tasks.push(
            Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(33)).await;
                },
                |_| Message::PollNativeKinectFrames,
            )
            .map(cosmic::Action::App),
        );

        Task::batch(tasks)
    }

    // ===== Depth Visualization Handlers =====

    /// Handle toggling depth colormap overlay
    pub(crate) fn handle_toggle_depth_overlay(&mut self) -> Task<cosmic::Action<Message>> {
        self.depth_viz.overlay_enabled = !self.depth_viz.overlay_enabled;
        // Update shared state for depth processing pipeline
        crate::shaders::depth::set_depth_colormap_enabled(self.depth_viz.overlay_enabled);
        debug!(
            enabled = self.depth_viz.overlay_enabled,
            "Toggled depth overlay"
        );
        Task::none()
    }

    /// Handle toggling depth grayscale mode
    pub(crate) fn handle_toggle_depth_grayscale(&mut self) -> Task<cosmic::Action<Message>> {
        self.depth_viz.grayscale_mode = !self.depth_viz.grayscale_mode;
        // Update shared state for depth processing pipeline
        crate::shaders::depth::set_depth_grayscale_mode(self.depth_viz.grayscale_mode);
        debug!(
            enabled = self.depth_viz.grayscale_mode,
            "Toggled depth grayscale mode"
        );
        Task::none()
    }

    // ===== Calibration Dialog Handlers =====

    /// Handle showing the calibration dialog
    pub(crate) fn handle_show_calibration_dialog(&mut self) -> Task<cosmic::Action<Message>> {
        self.kinect.calibration_dialog_visible = true;
        debug!("Showing calibration dialog");
        Task::none()
    }

    /// Handle closing the calibration dialog
    pub(crate) fn handle_close_calibration_dialog(&mut self) -> Task<cosmic::Action<Message>> {
        self.kinect.calibration_dialog_visible = false;
        debug!("Closing calibration dialog");
        Task::none()
    }

    /// Handle starting calibration fetch from device
    pub(crate) fn handle_start_calibration(&mut self) -> Task<cosmic::Action<Message>> {
        // Close dialog and attempt to fetch calibration from device
        self.kinect.calibration_dialog_visible = false;
        info!("Starting calibration fetch from device");
        // The actual calibration fetch happens when the Kinect backend is initialized
        // For now, we can re-trigger it by restarting the camera
        // TODO: Implement direct calibration fetch without restart
        Task::none()
    }

    // ===== 3D Preview Handlers =====

    /// Handle toggling the 3D preview mode
    pub(crate) fn handle_toggle_3d_preview(&mut self) -> Task<cosmic::Action<Message>> {
        self.preview_3d.enabled = !self.preview_3d.enabled;
        info!(
            enabled = self.preview_3d.enabled,
            is_kinect = self.kinect.is_device,
            has_depth = self.preview_3d.latest_depth_data.is_some(),
            depth_pixels = self
                .preview_3d
                .latest_depth_data
                .as_ref()
                .map(|(_, _, d)| d.len())
                .unwrap_or(0),
            "Toggled 3D preview"
        );
        if self.preview_3d.enabled {
            // Check if we need to start native Kinect streaming
            // (when on RGB camera and no depth data available)
            if self.kinect.is_device
                && self.preview_3d.latest_depth_data.is_none()
                && !self.is_current_camera_depth_sensor()
            {
                // Start native USB streaming for simultaneous RGB+depth
                info!("Starting native Kinect streaming for 3D preview");
                return Task::done(cosmic::Action::App(Message::StartNativeKinectStreaming));
            }
            // Trigger initial render when enabling 3D mode
            self.handle_request_point_cloud_render()
        } else {
            // Stop native Kinect streaming if running
            if self.kinect.streaming {
                return Task::done(cosmic::Action::App(Message::StopNativeKinectStreaming));
            }
            // Clear point cloud preview when disabling
            self.preview_3d.rendered_preview = None;
            Task::none()
        }
    }

    /// Handle toggling between point cloud and mesh view modes
    pub(crate) fn handle_toggle_scene_view_mode(&mut self) -> Task<cosmic::Action<Message>> {
        self.preview_3d.view_mode = match self.preview_3d.view_mode {
            SceneViewMode::PointCloud => SceneViewMode::Mesh,
            SceneViewMode::Mesh => SceneViewMode::PointCloud,
        };
        info!(mode = ?self.preview_3d.view_mode, "Toggled scene view mode");
        // Trigger re-render with new mode
        self.handle_request_point_cloud_render()
    }

    /// Handle mouse press on 3D preview (start dragging)
    pub(crate) fn handle_preview_3d_mouse_pressed(
        &mut self,
        _x: f32,
        _y: f32,
    ) -> Task<cosmic::Action<Message>> {
        // Store current rotation as base for this drag (path independence)
        self.preview_3d.base_rotation = self.preview_3d.rotation;
        self.preview_3d.dragging = true;
        // Don't set drag_start_pos here - on_press gives hardcoded (0,0)
        // First mouse move will set the actual start position
        self.preview_3d.drag_start_pos = None;
        self.preview_3d.last_mouse_pos = None;
        Task::none()
    }

    /// Handle mouse move on 3D preview (update rotation while dragging)
    pub(crate) fn handle_preview_3d_mouse_moved(
        &mut self,
        x: f32,
        y: f32,
    ) -> Task<cosmic::Action<Message>> {
        if self.preview_3d.dragging {
            // Check if this is the first move after press
            if self.preview_3d.drag_start_pos.is_none() {
                // First move - record actual start position
                // (on_press gives hardcoded 0,0 so we capture real position here)
                self.preview_3d.drag_start_pos = Some((x, y));
                return Task::none();
            }

            if let Some((start_x, start_y)) = self.preview_3d.drag_start_pos {
                // Calculate delta from START position (path independence)
                // This means dragging back to start returns to original view
                let delta_x = x - start_x;
                let delta_y = y - start_y;
                let (base_pitch, base_yaw) = self.preview_3d.base_rotation;

                // Adjust rotation sensitivity (radians per pixel)
                let sensitivity = 0.005;
                // Invert pitch so dragging down tilts view up (natural)
                // Clamp pitch to prevent flipping over
                let new_pitch = (base_pitch - delta_y * sensitivity).clamp(-1.4, 1.4);
                // Yaw rotates normally (drag right = rotate right)
                let new_yaw = base_yaw + delta_x * sensitivity;

                self.preview_3d.rotation = (new_pitch, new_yaw);
            }

            // Throttle render requests to ~60fps to prevent stuttering
            // The GPU render blocks, so limiting requests prevents frame stacking
            const RENDER_INTERVAL: Duration = Duration::from_millis(16);
            let now = Instant::now();
            let should_render = self
                .preview_3d
                .last_render_request_time
                .map(|last| now.duration_since(last) >= RENDER_INTERVAL)
                .unwrap_or(true);

            if should_render {
                self.preview_3d.last_render_request_time = Some(now);
                return self.handle_request_point_cloud_render();
            }
            return Task::none();
        }
        Task::none()
    }

    /// Handle mouse release on 3D preview (stop dragging)
    pub(crate) fn handle_preview_3d_mouse_released(&mut self) -> Task<cosmic::Action<Message>> {
        self.preview_3d.dragging = false;
        self.preview_3d.drag_start_pos = None;
        self.preview_3d.last_mouse_pos = None;
        // Current rotation now becomes the "resting" rotation for next drag
        // Re-render point cloud with final rotation
        self.handle_request_point_cloud_render()
    }

    /// Handle resetting 3D preview rotation and zoom to defaults
    pub(crate) fn handle_reset_3d_preview_rotation(&mut self) -> Task<cosmic::Action<Message>> {
        self.preview_3d.rotation = (0.0, 0.0);
        self.preview_3d.base_rotation = (0.0, 0.0);
        self.preview_3d.zoom = 0.0; // 0 = at sensor position (1x view)
        debug!("Reset 3D preview rotation and zoom");
        // Re-render point cloud with new rotation
        self.handle_request_point_cloud_render()
    }

    /// Handle zooming the 3D preview (scroll wheel)
    pub(crate) fn handle_zoom_3d_preview(&mut self, delta: f32) -> Task<cosmic::Action<Message>> {
        // Zoom in/out based on scroll delta - "fly into scene" model
        // Matches natural scrolling: scroll up = zoom in (like normal camera mode)
        // preview_3d_zoom = camera Z position (0 = at sensor, positive = into scene, negative = behind)
        // Scroll up (positive delta) = zoom in = move camera forward into scene
        // Scroll down (negative delta) = zoom out = move camera back
        // Range: -2.0 (2m behind sensor, zoomed out) to 3.0 (3m into scene, zoomed in)
        let zoom_sensitivity = 0.002;
        let new_zoom = (self.preview_3d.zoom + delta * zoom_sensitivity).clamp(-2.0, 3.0);
        self.preview_3d.zoom = new_zoom;
        debug!(zoom = new_zoom, "3D preview zoom changed");
        // Re-render point cloud with new zoom
        self.handle_request_point_cloud_render()
    }
}
