// SPDX-License-Identifier: MPL-2.0

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

            // Create icon container centered in 52x52 space
            let icon_content = widget::container(widget::icon(switch_icon).size(28))
                .width(Length::Fixed(52.0))
                .height(Length::Fixed(52.0))
                .center(52.0);

            // Create round button larger than gallery button for easy tapping
            let mut btn = widget::button::custom(icon_content)
                .padding(0)
                .width(Length::Fixed(52.0))
                .height(Length::Fixed(52.0))
                .class(cosmic::theme::Button::Icon);

            if !is_disabled {
                btn = btn.on_press(Message::SwitchCamera);
            }

            let button_element: Element<'_, Message> = btn.into();

            if is_disabled {
                // Wrap in container with reduced opacity when disabled
                widget::container(button_element)
                    .style(|_theme| widget::container::Style {
                        text_color: Some(cosmic::iced::Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                        ..Default::default()
                    })
                    .into()
            } else {
                button_element
            }
        } else {
            // Add invisible placeholder with same width as icon button
            widget::Space::new(Length::Fixed(ui::PLACEHOLDER_BUTTON_WIDTH), Length::Shrink).into()
        }
    }
}
