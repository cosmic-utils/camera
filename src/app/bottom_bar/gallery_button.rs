// SPDX-License-Identifier: MPL-2.0

//! Gallery button widget implementation

use std::sync::Arc;

use crate::app::gallery_widget::gallery_widget;
use crate::app::state::{AppModel, Message};
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, icon};

impl AppModel {
    /// Build the gallery button widget
    ///
    /// Shows a thumbnail if available, otherwise shows a folder icon.
    /// Disabled and grayed out during transitions.
    pub fn build_gallery_button(&self) -> Element<'_, Message> {
        let is_disabled = self.transition_state.ui_disabled;

        // If we have both the thumbnail handle and RGBA data, use custom primitive
        let button_content = if let (Some(thumbnail), Some((rgba_data, width, height))) =
            (&self.gallery_thumbnail, &self.gallery_thumbnail_rgba)
        {
            // Use custom GPU primitive with rounded corner clipping
            // Arc::clone is cheap - just increments reference count, doesn't copy image data
            gallery_widget(thumbnail.clone(), Arc::clone(rgba_data), *width, *height)
        } else if let Some(thumbnail) = &self.gallery_thumbnail {
            // Fallback: if we only have the handle (shouldn't happen in practice)
            let image = widget::image::Image::new(thumbnail.clone())
                .content_fit(cosmic::iced::ContentFit::Cover)
                .width(Length::Fixed(38.0))
                .height(Length::Fixed(38.0));

            widget::container(image)
                .width(Length::Fixed(40.0))
                .height(Length::Fixed(40.0))
                .into()
        } else {
            // Show folder icon when no thumbnail available
            widget::container(icon::from_name("folder-pictures-symbolic").size(24))
                .width(Length::Fixed(40.0))
                .height(Length::Fixed(40.0))
                .center(40.0)
                .into()
        };

        // Wrap in button with click handler
        let mut btn = widget::button::custom(button_content)
            .padding(0)
            .width(Length::Fixed(40.0))
            .height(Length::Fixed(40.0))
            .class(cosmic::theme::Button::Image);

        if !is_disabled {
            btn = btn.on_press(Message::OpenGallery);
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
    }
}
