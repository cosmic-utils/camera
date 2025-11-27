// SPDX-License-Identifier: MPL-2.0

//! Capture button widget implementation

use crate::app::state::{AppModel, CameraMode, Message};
use crate::constants::ui;
use cosmic::Element;
use cosmic::iced::{Background, Color, Length};
use cosmic::widget;

impl AppModel {
    /// Build the capture button widget
    ///
    /// The button changes appearance based on mode and state:
    /// - Photo mode: White circle (gray when capturing)
    /// - Video mode: Red circle (darker red when recording)
    /// - Press animation: Slightly smaller when active
    /// - Disabled: Grayed out and non-interactive during transitions
    pub fn build_capture_button(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let is_disabled = self.transition_state.ui_disabled;

        // Determine button color based on mode and state
        let capture_button_color = if is_disabled {
            Color::from_rgba(0.5, 0.5, 0.5, 0.3) // Grayed out with low opacity when disabled
        } else if self.mode == CameraMode::Video {
            if self.recording.is_recording() {
                Color::from_rgb(0.6, 0.05, 0.05) // Darker red when recording
            } else {
                Color::from_rgb(0.9, 0.1, 0.1) // Red for video mode
            }
        } else {
            if self.is_capturing {
                Color::from_rgb(0.7, 0.7, 0.7) // Gray when capturing
            } else {
                Color::WHITE // White for photo mode
            }
        };

        // Apply size changes based on state
        // - Recording: 70% smaller and stays that size while recording
        // - Capturing photo: 85% press down effect (brief)
        let (inner_size, outer_size) = if self.recording.is_recording() {
            (
                ui::CAPTURE_BUTTON_INNER * 0.70, // 70% smaller during recording
                ui::CAPTURE_BUTTON_OUTER * 0.70,
            )
        } else if self.is_capturing {
            (
                ui::CAPTURE_BUTTON_INNER * 0.85, // Press down effect for photo
                ui::CAPTURE_BUTTON_OUTER * 0.85,
            )
        } else {
            (ui::CAPTURE_BUTTON_INNER, ui::CAPTURE_BUTTON_OUTER)
        };

        let button_inner = widget::container(widget::Space::new(
            Length::Fixed(inner_size),
            Length::Fixed(inner_size),
        ))
        .style(move |_theme| widget::container::Style {
            background: Some(Background::Color(capture_button_color)),
            border: cosmic::iced::Border {
                radius: [ui::CAPTURE_BUTTON_RADIUS * (inner_size / ui::CAPTURE_BUTTON_INNER); 4]
                    .into(),
                ..Default::default()
            },
            ..Default::default()
        });

        let button = if is_disabled {
            // No on_press handler when disabled (non-clickable)
            widget::button::custom(button_inner)
                .padding(0)
                .width(Length::Fixed(outer_size))
                .height(Length::Fixed(outer_size))
        } else {
            // Normal interactive button
            widget::button::custom(button_inner)
                .on_press(match self.mode {
                    CameraMode::Photo => Message::Capture,
                    CameraMode::Video => Message::ToggleRecording,
                })
                .padding(0)
                .width(Length::Fixed(outer_size))
                .height(Length::Fixed(outer_size))
        };

        // Wrap button in a fixed-size container to prevent layout shift when button shrinks
        let button_wrapper = widget::container(button)
            .width(Length::Fixed(ui::CAPTURE_BUTTON_OUTER))
            .height(Length::Fixed(ui::CAPTURE_BUTTON_OUTER))
            .center_x(ui::CAPTURE_BUTTON_OUTER)
            .center_y(ui::CAPTURE_BUTTON_OUTER);

        widget::container(button_wrapper)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding([spacing.space_xs, 0])
            .into()
    }
}
