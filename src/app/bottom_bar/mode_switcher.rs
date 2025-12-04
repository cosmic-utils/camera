// SPDX-License-Identifier: GPL-3.0-only

//! Mode switcher widget implementation (Photo/Video/Virtual toggle)

use crate::app::state::{AppModel, CameraMode, Message};
use crate::app::view::overlay_container_style;
use crate::fl;
use cosmic::Element;
use cosmic::iced::Color;
use cosmic::widget;

/// Helper to wrap a mode button with overlay styling
fn styled_mode_button<'a>(
    button: impl Into<Element<'a, Message>>,
    is_disabled: bool,
) -> Element<'a, Message> {
    widget::container(button)
        .style(move |theme| {
            let mut style = overlay_container_style(theme);
            if is_disabled {
                style.text_color = Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3));
            }
            style
        })
        .into()
}

impl AppModel {
    /// Build the mode switcher widget
    ///
    /// Shows buttons for Photo, Video, and optionally Virtual modes.
    /// The active mode is highlighted with a suggested button style.
    /// Disabled and grayed out during transitions, recording, or streaming.
    /// Virtual mode button is only shown when virtual_camera_enabled is true.
    pub fn build_mode_switcher(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        // Disable mode switching during transitions, recording, or streaming
        let is_disabled = self.transition_state.ui_disabled
            || self.recording.is_recording()
            || self.virtual_camera.is_streaming();

        // Use Suggested for active mode, Text for inactive - COSMIC's native button highlighting
        let video_label = fl!("mode-video");
        let video_button = if is_disabled {
            widget::button::text(video_label).class(if self.mode == CameraMode::Video {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
        } else {
            widget::button::text(video_label)
                .on_press(Message::SetMode(CameraMode::Video))
                .class(if self.mode == CameraMode::Video {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Text
                })
        };

        let photo_label = fl!("mode-photo");
        let photo_button = if is_disabled {
            widget::button::text(photo_label).class(if self.mode == CameraMode::Photo {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
        } else {
            widget::button::text(photo_label)
                .on_press(Message::SetMode(CameraMode::Photo))
                .class(if self.mode == CameraMode::Photo {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Text
                })
        };

        let mut row = widget::row()
            .push(styled_mode_button(video_button, is_disabled))
            .push(widget::horizontal_space().width(spacing.space_xs))
            .push(styled_mode_button(photo_button, is_disabled))
            .spacing(spacing.space_xxs);

        // Only show Virtual button when the feature is enabled
        if self.config.virtual_camera_enabled {
            let virtual_label = fl!("mode-virtual");
            let virtual_button = if is_disabled {
                widget::button::text(virtual_label).class(if self.mode == CameraMode::Virtual {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Text
                })
            } else {
                widget::button::text(virtual_label)
                    .on_press(Message::SetMode(CameraMode::Virtual))
                    .class(if self.mode == CameraMode::Virtual {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Text
                    })
            };

            row = row
                .push(widget::horizontal_space().width(spacing.space_xs))
                .push(styled_mode_button(virtual_button, is_disabled));
        }

        row.into()
    }
}
