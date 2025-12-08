// SPDX-License-Identifier: GPL-3.0-only

//! Color control handlers
//!
//! Handles color adjustment controls including contrast, saturation, sharpness,
//! hue, and white balance.

use crate::app::state::{AppModel, Message};
use crate::backends::camera::v4l2_controls;
use cosmic::Task;
use tracing::{debug, info};

impl AppModel {
    // =========================================================================
    // Color Control Handlers
    // =========================================================================

    pub(crate) fn handle_toggle_color_picker(&mut self) -> Task<cosmic::Action<Message>> {
        let opening = !self.color_picker_visible;
        self.close_all_pickers();
        self.color_picker_visible = opening;
        info!(visible = self.color_picker_visible, "Color picker toggled");
        Task::none()
    }

    pub(crate) fn handle_close_color_picker(&mut self) -> Task<cosmic::Action<Message>> {
        self.color_picker_visible = false;
        Task::none()
    }

    pub(crate) fn handle_set_contrast(&mut self, value: i32) -> Task<cosmic::Action<Message>> {
        if let Some(ref mut settings) = self.color_settings {
            settings.contrast = Some(value);
        }
        debug!(value, "Setting contrast");
        self.set_v4l2_control(v4l2_controls::V4L2_CID_CONTRAST, value)
    }

    pub(crate) fn handle_set_saturation(&mut self, value: i32) -> Task<cosmic::Action<Message>> {
        if let Some(ref mut settings) = self.color_settings {
            settings.saturation = Some(value);
        }
        debug!(value, "Setting saturation");
        self.set_v4l2_control(v4l2_controls::V4L2_CID_SATURATION, value)
    }

    pub(crate) fn handle_set_sharpness(&mut self, value: i32) -> Task<cosmic::Action<Message>> {
        if let Some(ref mut settings) = self.color_settings {
            settings.sharpness = Some(value);
        }
        debug!(value, "Setting sharpness");
        self.set_v4l2_control(v4l2_controls::V4L2_CID_SHARPNESS, value)
    }

    pub(crate) fn handle_set_hue(&mut self, value: i32) -> Task<cosmic::Action<Message>> {
        if let Some(ref mut settings) = self.color_settings {
            settings.hue = Some(value);
        }
        debug!(value, "Setting hue");
        self.set_v4l2_control(v4l2_controls::V4L2_CID_HUE, value)
    }

    pub(crate) fn handle_toggle_auto_white_balance(&mut self) -> Task<cosmic::Action<Message>> {
        let current = self
            .color_settings
            .as_ref()
            .and_then(|s| s.white_balance_auto)
            .unwrap_or(true);
        let new_value = !current;

        if let Some(ref mut settings) = self.color_settings {
            settings.white_balance_auto = Some(new_value);
        }

        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        info!(enabled = new_value, "Toggling auto white balance");

        // When switching from auto to manual, read current temperature and apply it
        let has_temp_control = self
            .available_exposure_controls
            .white_balance_temperature
            .available;
        let switching_to_manual = !new_value && has_temp_control;

        Task::perform(
            async move {
                // First, disable auto white balance
                v4l2_controls::set_control(
                    &device_path,
                    v4l2_controls::V4L2_CID_AUTO_WHITE_BALANCE,
                    if new_value { 1 } else { 0 },
                )?;

                // When switching to manual, read current temp and set it
                // This preserves the temperature that auto mode was using
                if switching_to_manual {
                    if let Some(current_temp) = v4l2_controls::get_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_WHITE_BALANCE_TEMPERATURE,
                    ) {
                        v4l2_controls::set_control(
                            &device_path,
                            v4l2_controls::V4L2_CID_WHITE_BALANCE_TEMPERATURE,
                            current_temp,
                        )?;
                        return Ok(Some(current_temp));
                    }
                }
                Ok(None)
            },
            |result: Result<Option<i32>, String>| {
                cosmic::Action::App(match result {
                    Ok(temp) => Message::WhiteBalanceToggled(temp),
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }

    pub(crate) fn handle_set_white_balance_temperature(
        &mut self,
        value: i32,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(ref mut settings) = self.color_settings {
            settings.white_balance_temperature = Some(value);
        }
        debug!(temperature = value, "Setting white balance temperature");
        self.set_v4l2_control(v4l2_controls::V4L2_CID_WHITE_BALANCE_TEMPERATURE, value)
    }

    pub(crate) fn handle_reset_color_settings(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Resetting color settings to defaults");
        self.reset_color_settings_to_defaults()
    }

    /// Reset color settings to defaults (helper for filter selection and reset button)
    pub(crate) fn reset_color_settings_to_defaults(&mut self) -> Task<cosmic::Action<Message>> {
        let controls = &self.available_exposure_controls;
        let Some(device_path) = self.get_v4l2_device_path() else {
            return Task::none();
        };

        // Reset local state to defaults
        if let Some(ref mut settings) = self.color_settings {
            if controls.contrast.available {
                settings.contrast = Some(controls.contrast.default);
            }
            if controls.saturation.available {
                settings.saturation = Some(controls.saturation.default);
            }
            if controls.sharpness.available {
                settings.sharpness = Some(controls.sharpness.default);
            }
            if controls.hue.available {
                settings.hue = Some(controls.hue.default);
            }
            if controls.has_white_balance_auto {
                settings.white_balance_auto = Some(true);
            }
            if controls.white_balance_temperature.available {
                settings.white_balance_temperature =
                    Some(controls.white_balance_temperature.default);
            }
        }

        // Apply defaults to camera
        let contrast_default = controls.contrast.default;
        let saturation_default = controls.saturation.default;
        let sharpness_default = controls.sharpness.default;
        let hue_default = controls.hue.default;
        let wb_temp_default = controls.white_balance_temperature.default;
        let has_contrast = controls.contrast.available;
        let has_saturation = controls.saturation.available;
        let has_sharpness = controls.sharpness.available;
        let has_hue = controls.hue.available;
        let has_wb_auto = controls.has_white_balance_auto;
        let has_wb_temp = controls.white_balance_temperature.available;

        debug!("Resetting color settings to defaults for filter");
        Task::perform(
            async move {
                if has_contrast {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_CONTRAST,
                        contrast_default,
                    );
                }
                if has_saturation {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_SATURATION,
                        saturation_default,
                    );
                }
                if has_sharpness {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_SHARPNESS,
                        sharpness_default,
                    );
                }
                if has_hue {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_HUE,
                        hue_default,
                    );
                }
                if has_wb_auto {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_AUTO_WHITE_BALANCE,
                        1,
                    );
                }
                if has_wb_temp {
                    let _ = v4l2_controls::set_control(
                        &device_path,
                        v4l2_controls::V4L2_CID_WHITE_BALANCE_TEMPERATURE,
                        wb_temp_default,
                    );
                }
                Ok::<(), String>(())
            },
            |result| {
                cosmic::Action::App(match result {
                    Ok(_) => Message::ExposureControlApplied,
                    Err(e) => Message::ExposureControlFailed(e),
                })
            },
        )
    }
}
