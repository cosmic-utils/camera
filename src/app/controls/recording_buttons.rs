// SPDX-License-Identifier: MPL-2.0

//! Recording control buttons (photo during recording)

use crate::app::state::{AppModel, Message};
use crate::constants::ui;
use cosmic::Element;
use cosmic::iced::{Background, Color, Length};
use cosmic::widget;

impl AppModel {
    /// Build the photo capture button (shown during recording on the right)
    pub fn build_photo_during_recording_button(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        let size = ui::CAPTURE_BUTTON_INNER * 0.70;

        // White circle with smaller inner circle for photo capture
        let inner_circle_size = size * 0.85;
        let button_inner = widget::container(widget::Space::new(
            Length::Fixed(inner_circle_size),
            Length::Fixed(inner_circle_size),
        ))
        .style(move |_theme| widget::container::Style {
            background: Some(Background::Color(Color::WHITE)),
            border: cosmic::iced::Border {
                radius: [ui::CAPTURE_BUTTON_RADIUS * 0.70 * 0.85; 4].into(),
                ..Default::default()
            },
            ..Default::default()
        });

        let button_outer = widget::container(button_inner)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .center_x(size)
            .center_y(size)
            .style(move |_theme| widget::container::Style {
                background: Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.3))),
                border: cosmic::iced::Border {
                    radius: [ui::CAPTURE_BUTTON_RADIUS * 0.70; 4].into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        let button = widget::button::custom(button_outer)
            .on_press(Message::Capture)
            .padding(0)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size));

        // Wrap in fixed-size container to match capture button layout
        let button_wrapper = widget::container(button)
            .width(Length::Fixed(ui::CAPTURE_BUTTON_OUTER))
            .height(Length::Fixed(ui::CAPTURE_BUTTON_OUTER))
            .center_x(ui::CAPTURE_BUTTON_OUTER)
            .center_y(ui::CAPTURE_BUTTON_OUTER);

        widget::container(button_wrapper)
            .padding([spacing.space_xs, 0])
            .into()
    }
}
