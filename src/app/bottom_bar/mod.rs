// SPDX-License-Identifier: MPL-2.0

//! Bottom bar module
//!
//! This module handles the bottom control bar UI components:
//! - Gallery button (with thumbnail)
//! - Mode switcher (Photo/Video toggle)
//! - Camera switcher (flip cameras)

pub mod camera_switcher;
pub mod gallery_button;
pub mod mode_switcher;

// Re-export for convenience

use crate::app::state::{AppModel, Message};
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget;

/// Fixed height for bottom bar to match filter picker
const BOTTOM_BAR_HEIGHT: f32 = 68.0;

impl AppModel {
    /// Build the complete bottom bar widget
    ///
    /// Assembles gallery button, mode switcher, and camera switcher
    /// into a centered horizontal layout.
    pub fn build_bottom_bar(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // Build the centered group: gallery button + mode selector + camera switch button
        let mut centered_group = widget::row();

        centered_group = centered_group.push(self.build_gallery_button());

        centered_group = centered_group
            .push(widget::horizontal_space().width(spacing.space_m))
            .push(self.build_mode_switcher())
            .push(widget::horizontal_space().width(spacing.space_m))
            .align_y(Alignment::Center);

        centered_group = centered_group.push(self.build_camera_switcher());

        // Center the entire group in the bottom bar
        let bottom_row = widget::row()
            .push(widget::Space::new(Length::Fill, Length::Shrink))
            .push(centered_group)
            .push(widget::Space::new(Length::Fill, Length::Shrink))
            .padding(spacing.space_xs)
            .align_y(Alignment::Center);

        widget::container(bottom_row)
            .width(Length::Fill)
            .height(Length::Fixed(BOTTOM_BAR_HEIGHT))
            .center_y(BOTTOM_BAR_HEIGHT)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            })
            .into()
    }
}
