// SPDX-License-Identifier: GPL-3.0-only

//! Exposure control handlers
//!
//! Handles exposure mode, compensation, time, gain, ISO, metering, and backlight.

use crate::app::exposure_picker::{
    AvailableExposureControls, ColorSettings, ExposureMode, ExposureSettings, MeteringMode,
};
use crate::app::state::{AppModel, Message};
use crate::backends::camera::v4l2_controls;
use cosmic::Task;
use tracing::{debug, info};

impl AppModel {
    // =========================================================================
    // Exposure Control Handlers
    // =========================================================================

    pub(crate) fn handle_toggle_exposure_picker(&mut self) -> Task<cosmic::Action<Message>> {
        let opening = !self.exposure_picker_visible;
        self.close_all_pickers();
        self.exposure_picker_visible = opening;
        if opening {
            // Clear base exposure time when opening (will be captured on first slider move)
            self.base_exposure_time = None;
        }
        info!(
            visible = self.exposure_picker_visible,
            "Exposure picker toggled"
        );
        Task::none()
    }

    pub(crate) fn handle_close_exposure_picker(&mut self) -> Task<cosmic::Action<Message>> {
        self.exposure_picker_visible = false;
        Task::none()
    }

    pub(crate) fn handle_set_exposure_mode(
        &mut self,
        mode: ExposureMode,
    ) -> Task<cosmic::Action<Message>> {
        // Update local state
        if let Some(ref mut settings) = self.exposure_settings {
            settings.mode = mode;
        }

        // Sync segmented button model
        let position = if mode == ExposureMode::Manual { 1 } else { 0 };
        self.exposure_mode_model.activate_position(position);

        // Apply to camera via V4L2
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        let value = mode.to_v4l2_value();
        info!(mode = ?mode, value, "Setting exposure mode");

        Task::perform(
            async move {
                v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_EXPOSURE_AUTO,
                    value,
                )
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_exposure_mode_selected(
        &mut self,
        entity: cosmic::widget::segmented_button::Entity,
    ) -> Task<cosmic::Action<Message>> {
        // Activate the selected button
        self.exposure_mode_model.activate(entity);

        // Get position to determine mode (0 = Auto, 1 = Manual)
        let position = self.exposure_mode_model.position(entity).unwrap_or(0);
        let mode = if position == 0 {
            ExposureMode::AperturePriority
        } else {
            ExposureMode::Manual
        };

        self.handle_set_exposure_mode(mode)
    }

    pub(crate) fn handle_set_exposure_compensation(
        &mut self,
        value: i32,
    ) -> Task<cosmic::Action<Message>> {
        // Update local state
        if let Some(ref mut settings) = self.exposure_settings {
            settings.exposure_compensation = value;
        }

        // Apply to camera via V4L2
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        debug!(
            ev_units = value,
            ev = value as f32 / 1000.0,
            "Setting exposure compensation"
        );

        // Set EV bias directly (only works on cameras that support it)
        Task::perform(
            async move {
                v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_AUTO_EXPOSURE_BIAS,
                    value,
                )
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_reset_exposure_compensation(&mut self) -> Task<cosmic::Action<Message>> {
        // Reset local state
        if let Some(ref mut settings) = self.exposure_settings {
            settings.exposure_compensation = 0;
        }

        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        info!("Resetting exposure compensation");

        // Reset EV bias to 0 (only works on cameras that support it)
        Task::perform(
            async move {
                v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_AUTO_EXPOSURE_BIAS,
                    0,
                )
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_set_exposure_time(&mut self, value: i32) -> Task<cosmic::Action<Message>> {
        // Update local state
        if let Some(ref mut settings) = self.exposure_settings {
            settings.exposure_time = Some(value);
        }

        // Apply to camera via V4L2
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        // Get current gain to re-apply (some cameras need both set together)
        let current_gain = self
            .exposure_settings
            .as_ref()
            .and_then(|s| s.gain)
            .unwrap_or(self.available_exposure_controls.gain.default);
        let has_gain = self.available_exposure_controls.gain.available;

        debug!(
            time_100us = value,
            gain = current_gain,
            "Setting exposure time"
        );

        // Exposure time only works in manual mode - ensure we're in manual mode first
        let manual_value = ExposureMode::Manual.to_v4l2_value();

        Task::perform(
            async move {
                // Ensure manual mode is set (some cameras need this before each exposure change)
                let _ = v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_EXPOSURE_AUTO,
                    manual_value,
                );

                // Set the exposure time
                let result = v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_EXPOSURE_ABSOLUTE,
                    value,
                );

                // Also re-apply gain (some cameras need both set together for changes to take effect)
                if has_gain {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_GAIN,
                        current_gain,
                    );
                }

                result
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_set_gain(&mut self, value: i32) -> Task<cosmic::Action<Message>> {
        // Update local state
        if let Some(ref mut settings) = self.exposure_settings {
            settings.gain = Some(value);
        }

        // Apply to camera via V4L2
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        // Get current exposure time to re-apply (some cameras need both set together)
        let current_exposure = self
            .exposure_settings
            .as_ref()
            .and_then(|s| s.exposure_time)
            .unwrap_or(self.available_exposure_controls.exposure_time.default);
        let has_exposure = self.available_exposure_controls.exposure_time.available;

        debug!(gain = value, exposure = current_exposure, "Setting gain");

        // Ensure manual mode for gain control
        let manual_value = ExposureMode::Manual.to_v4l2_value();

        Task::perform(
            async move {
                // Ensure manual mode is set
                let _ = v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_EXPOSURE_AUTO,
                    manual_value,
                );

                // Try standard gain first, then analogue gain
                let result =
                    v4l2_controls::set_control(&device_path, v4l2_controls::V4L2_CID_GAIN, value);
                let result = if result.is_err() {
                    v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_ANALOGUE_GAIN,
                        value,
                    )
                } else {
                    result
                };

                // Also re-apply exposure time (some cameras need both set together)
                if has_exposure {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_EXPOSURE_ABSOLUTE,
                        current_exposure,
                    );
                }

                result
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_set_iso_sensitivity(
        &mut self,
        value: i32,
    ) -> Task<cosmic::Action<Message>> {
        // Update local state
        if let Some(ref mut settings) = self.exposure_settings {
            settings.iso = Some(value);
        }

        // Apply to camera via V4L2
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        debug!(iso = value, "Setting ISO sensitivity");

        Task::perform(
            async move {
                v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_ISO_SENSITIVITY,
                    value,
                )
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_set_metering_mode(
        &mut self,
        mode: MeteringMode,
    ) -> Task<cosmic::Action<Message>> {
        // Update local state
        if let Some(ref mut settings) = self.exposure_settings {
            settings.metering_mode = Some(mode);
        }

        // Apply to camera via V4L2
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        let value = mode.to_v4l2_value();
        debug!(mode = ?mode, value, "Setting metering mode");

        Task::perform(
            async move {
                v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_EXPOSURE_METERING,
                    value,
                )
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_toggle_auto_exposure_priority(&mut self) -> Task<cosmic::Action<Message>> {
        // Update local state
        let new_value = if let Some(ref mut settings) = self.exposure_settings {
            let current = settings.auto_priority.unwrap_or(false);
            settings.auto_priority = Some(!current);
            !current
        } else {
            true
        };

        // Apply to camera via V4L2
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        let value = if new_value { 1 } else { 0 };
        debug!(enabled = new_value, "Setting auto exposure priority");

        Task::perform(
            async move {
                v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_EXPOSURE_AUTO_PRIORITY,
                    value,
                )
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_set_backlight_compensation(
        &mut self,
        value: i32,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(ref mut settings) = self.exposure_settings {
            settings.backlight_compensation = Some(value);
        }
        debug!(backlight = value, "Setting backlight compensation");
        self.set_v4l2_control(v4l2_controls::V4L2_CID_BACKLIGHT_COMPENSATION, value)
    }

    pub(crate) fn handle_set_focus_absolute(
        &mut self,
        value: i32,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(ref mut settings) = self.exposure_settings {
            settings.focus_absolute = Some(value);
        }

        let focus_path = self
            .get_focus_device_path()
            .or_else(|| self.get_v4l2_device_path());
        let Some(focus_path) = focus_path else {
            return Task::none();
        };

        debug!(focus = value, path = %focus_path, "Setting focus absolute");

        Task::perform(
            async move {
                v4l2_controls::set_control(
                    &focus_path,
                    v4l2_controls::V4L2_CID_FOCUS_ABSOLUTE,
                    value,
                )
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_toggle_focus_auto(&mut self) -> Task<cosmic::Action<Message>> {
        let new_value = if let Some(ref mut settings) = self.exposure_settings {
            let current = settings.focus_auto.unwrap_or(false);
            settings.focus_auto = Some(!current);
            !current
        } else {
            true
        };

        let focus_path = self
            .get_focus_device_path()
            .or_else(|| self.get_v4l2_device_path());
        let Some(focus_path) = focus_path else {
            return Task::none();
        };

        let value = if new_value { 1 } else { 0 };
        debug!(enabled = new_value, path = %focus_path, "Setting focus auto");

        Task::perform(
            async move {
                v4l2_controls::set_control(&focus_path, v4l2_controls::V4L2_CID_FOCUS_AUTO, value)
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    /// Reset all exposure settings to camera defaults (preserving current mode)
    pub(crate) fn handle_reset_exposure_settings(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        let controls = self.available_exposure_controls.clone();
        let current_mode = self
            .exposure_settings
            .as_ref()
            .map(|s| s.mode)
            .unwrap_or_default();

        info!(?current_mode, "Resetting exposure settings to defaults");

        // Update local state to defaults
        if let Some(ref mut settings) = self.exposure_settings {
            settings.exposure_compensation = controls.exposure_bias.default;
            settings.exposure_time = Some(controls.exposure_time.default);
            settings.gain = Some(controls.gain.default);
            settings.iso = Some(controls.iso.default);
            settings.backlight_compensation = Some(controls.backlight_compensation.default);
            settings.auto_priority = Some(false);
            // Keep mode unchanged
        }

        // Apply defaults to camera
        Task::perform(
            async move {
                // Reset exposure time
                if controls.exposure_time.available {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_EXPOSURE_ABSOLUTE,
                        controls.exposure_time.default,
                    );
                }

                // Reset gain
                if controls.gain.available {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_GAIN,
                        controls.gain.default,
                    );
                }

                // Reset ISO
                if controls.iso.available {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_ISO_SENSITIVITY,
                        controls.iso.default,
                    );
                }

                // Reset exposure bias/compensation
                if controls.exposure_bias.available {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_AUTO_EXPOSURE_BIAS,
                        controls.exposure_bias.default,
                    );
                }

                // Reset backlight compensation
                if controls.backlight_compensation.available {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_BACKLIGHT_COMPENSATION,
                        controls.backlight_compensation.default,
                    );
                }

                // Reset auto priority
                if controls.has_auto_priority {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_EXPOSURE_AUTO_PRIORITY,
                        0,
                    );
                }
            },
            |_| cosmic::Action::App(Message::ExposureControlApplied),
        )
    }

    pub(crate) fn handle_exposure_controls_queried(
        &mut self,
        controls: Box<AvailableExposureControls>,
        settings: ExposureSettings,
        color_settings: ColorSettings,
    ) -> Task<cosmic::Action<Message>> {
        info!(
            has_mode = controls.has_exposure_auto,
            has_ev = controls.exposure_bias.available,
            has_time = controls.exposure_time.available,
            has_gain = controls.gain.available,
            has_iso = controls.iso.available,
            "Exposure controls queried"
        );
        self.available_exposure_controls = *controls;
        self.exposure_settings = Some(settings);
        self.color_settings = Some(color_settings);
        Task::none()
    }

    // =========================================================================
    // V4L2 Helpers (used by exposure and color handlers)
    // =========================================================================

    /// Get the V4L2 device path for the current camera
    pub(crate) fn get_v4l2_device_path(&self) -> Option<String> {
        self.available_cameras
            .get(self.current_camera_index)
            .and_then(|cam| cam.device_info.as_ref())
            .map(|info| info.path.clone())
    }

    /// Helper to set a V4L2 control value asynchronously
    ///
    /// This reduces duplication across exposure and color control handlers.
    pub(crate) fn set_v4l2_control(
        &self,
        control_id: u32,
        value: i32,
    ) -> Task<cosmic::Action<Message>> {
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        Task::perform(
            async move { v4l2_controls::set_control(&device_path, control_id, value) },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    /// Get the lens actuator device path for the current camera
    pub(crate) fn get_focus_device_path(&self) -> Option<String> {
        self.available_cameras
            .get(self.current_camera_index)
            .and_then(|cam| cam.lens_actuator_path.clone())
    }

    /// Create a task to query exposure controls for the current camera
    /// This resets exposure settings to defaults (aperture priority mode, default backlight)
    pub(crate) fn query_exposure_controls_task(&self) -> Task<cosmic::Action<Message>> {
        if let Some(device_path) = self.get_v4l2_device_path() {
            let path = device_path.clone();
            let focus_path = self.get_focus_device_path();
            Task::perform(
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
            )
        } else {
            Task::none()
        }
    }
}
