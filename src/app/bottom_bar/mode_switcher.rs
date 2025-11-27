// SPDX-License-Identifier: MPL-2.0

//! Mode switcher widget implementation (Photo/Video toggle)

use crate::app::state::{AppModel, CameraMode, Message};
use cosmic::Element;
use cosmic::widget;

impl AppModel {
    /// Build the mode switcher widget
    ///
    /// Shows two buttons for Photo and Video modes.
    /// The active mode is highlighted with a suggested button style.
    /// Disabled and grayed out during transitions.
    pub fn build_mode_switcher(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let is_disabled = self.transition_state.ui_disabled;

        let video_button = if is_disabled {
            // Disabled during transitions - no action
            widget::button::text("VIDEO").class(if self.mode == CameraMode::Video {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
        } else {
            // Always has on_press, but SetMode handler checks if mode actually changes
            widget::button::text("VIDEO")
                .on_press(Message::SetMode(CameraMode::Video))
                .class(if self.mode == CameraMode::Video {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Text
                })
        };

        let photo_button = if is_disabled {
            // Disabled during transitions - no action
            widget::button::text("PHOTO").class(if self.mode == CameraMode::Photo {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
        } else {
            // Always has on_press, but SetMode handler checks if mode actually changes
            widget::button::text("PHOTO")
                .on_press(Message::SetMode(CameraMode::Photo))
                .class(if self.mode == CameraMode::Photo {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Text
                })
        };

        let row = widget::row()
            .push(video_button)
            .push(widget::horizontal_space().width(spacing.space_xs))
            .push(photo_button)
            .spacing(spacing.space_xxs);

        if is_disabled {
            // Wrap in container with reduced opacity when disabled
            widget::container(row)
                .style(|_theme| widget::container::Style {
                    text_color: Some(cosmic::iced::Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                    ..Default::default()
                })
                .into()
        } else {
            row.into()
        }
    }
}
