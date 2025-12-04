// SPDX-License-Identifier: GPL-3.0-only

//! Camera switcher button widget implementation

use crate::app::state::{AppModel, Message};
use crate::app::view::overlay_container_style;
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
    /// Disabled and grayed out during transitions.
    /// Hidden during virtual camera streaming (camera cannot be switched while streaming).
    pub fn build_camera_switcher(&self) -> Element<'_, Message> {
        let is_disabled = self.transition_state.ui_disabled;

        // Hide camera switcher during virtual camera streaming
        if self.virtual_camera.is_streaming() {
            return widget::Space::new(Length::Fixed(ui::PLACEHOLDER_BUTTON_WIDTH), Length::Shrink)
                .into();
        }

        if self.available_cameras.len() > 1 {
            let switch_icon = widget::icon::from_svg_bytes(CAMERA_SWITCH_ICON).symbolic(true);

            // Create icon widget that inherits theme colors
            let icon_widget = widget::icon(switch_icon).size(32);

            // Center icon in fixed-size container
            let icon_content = widget::container(icon_widget)
                .width(Length::Fixed(52.0))
                .height(Length::Fixed(52.0))
                .center(Length::Fixed(52.0));

            // Use custom button with icon as content - matches top bar overlay_icon_button pattern
            // Use Button::Text for theme-aware styling (transparent background, themed icon color)
            let mut btn = widget::button::custom(icon_content)
                .padding(0)
                .class(cosmic::theme::Button::Text);

            if !is_disabled {
                btn = btn.on_press(Message::SwitchCamera);
            }

            // Wrap in container with themed background for better visibility on camera preview
            widget::container(btn).style(overlay_container_style).into()
        } else {
            // Add invisible placeholder with same width as icon button
            widget::Space::new(Length::Fixed(ui::PLACEHOLDER_BUTTON_WIDTH), Length::Shrink).into()
        }
    }
}
