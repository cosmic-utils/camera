// SPDX-License-Identifier: GPL-3.0-only

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
    /// into a layout where the mode switcher is horizontally centered,
    /// aligning with the capture button above.
    pub fn build_bottom_bar(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // Use three-column layout to ensure mode switcher is truly centered:
        // [left Fill + gallery] [mode_switcher] [camera_switcher + right Fill]
        // This ensures center alignment regardless of asymmetric button widths

        // Left section: Fill space + gallery button (right-aligned within the fill)
        let left_section = widget::row()
            .push(widget::Space::new(Length::Fill, Length::Shrink))
            .push(self.build_gallery_button())
            .push(widget::horizontal_space().width(spacing.space_m))
            .align_y(Alignment::Center);

        // Center section: mode switcher (truly centered in window)
        let center_section = self.build_mode_switcher();

        // Right section: camera switcher + Fill space (left-aligned within the fill)
        let right_section = widget::row()
            .push(widget::horizontal_space().width(spacing.space_m))
            .push(self.build_camera_switcher())
            .push(widget::Space::new(Length::Fill, Length::Shrink))
            .align_y(Alignment::Center);

        let bottom_row = widget::row()
            .push(left_section)
            .push(center_section)
            .push(right_section)
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
