// SPDX-License-Identifier: GPL-3.0-only

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

        // Get corner radius from theme — cap at half button size
        let theme = cosmic::theme::active();
        let corner_radius = theme.cosmic().corner_radii.radius_xl[0].min(22.0);

        // If we have both the thumbnail handle and RGBA data, use custom primitive
        let button_content = if let (Some(thumbnail), Some((rgba_data, width, height))) =
            (&self.gallery_thumbnail, &self.gallery_thumbnail_rgba)
        {
            gallery_widget(
                thumbnail.clone(),
                Arc::clone(rgba_data),
                *width,
                *height,
                corner_radius,
            )
        } else if let Some(thumbnail) = &self.gallery_thumbnail {
            let image = widget::image::Image::new(thumbnail.clone())
                .content_fit(cosmic::iced::ContentFit::Cover)
                .width(Length::Fixed(42.0))
                .height(Length::Fixed(42.0));

            widget::container(image)
                .width(Length::Fixed(44.0))
                .height(Length::Fixed(44.0))
                .into()
        } else {
            widget::container(icon::from_name("folder-pictures-symbolic").size(24))
                .width(Length::Fixed(44.0))
                .height(Length::Fixed(44.0))
                .center(44.0)
                .into()
        };

        // No button widget — the shader handles hover overlay directly.
        // Use mouse_area for click handling only.
        if is_disabled {
            button_content
        } else {
            widget::mouse_area(button_content)
                .on_press(Message::OpenGallery)
                .interaction(cosmic::iced::mouse::Interaction::Pointer)
                .into()
        }
    }
}
