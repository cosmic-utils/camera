// SPDX-License-Identifier: GPL-3.0-only

//! Camera switcher button widget implementation

use crate::app::state::{AppModel, Message};
use crate::constants::ui;
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget;

/// Camera switch icon SVG (camera with circular arrows)
const CAMERA_SWITCH_ICON: &[u8] =
    include_bytes!("../../../resources/button_icons/camera-switch.svg");

impl AppModel {
    /// Build the camera switcher button widget
    ///
    /// Shows a flip button if multiple cameras are available,
    /// otherwise shows an invisible placeholder to maintain consistent layout.
    /// Disabled and grayed out during transitions and recording.
    /// Hidden during virtual camera streaming (camera cannot be switched while streaming).
    pub fn build_camera_switcher(&self) -> Element<'_, Message> {
        let is_disabled = self.transition_state.ui_disabled
            || self.recording.is_recording()
            || self.quick_record.is_recording()
            || self.timelapse.is_active();

        // Hide camera switcher during virtual camera streaming
        if self.virtual_camera.is_streaming() {
            return widget::Space::new()
                .width(Length::Fixed(ui::PLACEHOLDER_BUTTON_WIDTH))
                .height(Length::Shrink)
                .into();
        }

        if self.available_cameras.len() > 1 {
            let switch_handle = widget::icon::from_svg_bytes(CAMERA_SWITCH_ICON).symbolic(true);

            // Create icon widget with accent color
            let icon_widget =
                widget::icon(switch_handle)
                    .size(24)
                    .class(cosmic::theme::Svg::custom(|theme| {
                        cosmic::iced::widget::svg::Style {
                            color: Some(cosmic::iced::Color::from(theme.cosmic().accent_color())),
                        }
                    }));

            let mut btn = widget::button::custom(icon_widget)
                .padding(10)
                .width(Length::Fixed(44.0))
                .height(Length::Fixed(44.0))
                .class(cosmic::theme::Button::Standard);

            if !is_disabled {
                btn = btn.on_press(Message::SwitchCamera);
            }

            btn.into()
        } else {
            widget::Space::new()
                .width(Length::Fixed(ui::PLACEHOLDER_BUTTON_WIDTH))
                .height(Length::Shrink)
                .into()
        }
    }
}
