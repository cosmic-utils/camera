// SPDX-License-Identifier: GPL-3.0-only

//! Bottom bar module
//!
//! This module handles the bottom control bar UI components:
//! - Gallery button (with thumbnail)
//! - Mode switcher (Photo/Video toggle)
//! - Camera switcher (flip cameras)

pub mod camera_switcher;
pub mod fade_primitive;
pub mod gallery_button;
pub mod mode_carousel;
pub mod mode_switcher;
pub mod slide_h;

// Re-export for convenience

use crate::app::state::{AppModel, Message};
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget;

use slide_h::SlideH;

/// Fixed height for bottom bar to match filter picker
pub const BOTTOM_BAR_HEIGHT: f32 = 74.0;

/// Shared horizontal layout used by the bottom bar (gallery / mode-carousel /
/// camera-switcher) and the recording-state capture row (spacer / stop circle
/// / photo button). The shape `[left] [Fill] [center] [Fill] [right]` keeps
/// the two layouts visually aligned column-for-column.
pub fn three_col_row<'a>(
    left: Element<'a, Message>,
    center: Element<'a, Message>,
    right: Element<'a, Message>,
    padding: impl Into<cosmic::iced::Padding>,
) -> Element<'a, Message> {
    let fill = || {
        widget::Space::new()
            .width(Length::Fill)
            .height(Length::Shrink)
    };
    widget::row()
        .push(left)
        .push(fill())
        .push(center)
        .push(fill())
        .push(right)
        .padding(padding)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

impl AppModel {
    /// Build the complete bottom bar widget
    ///
    /// Assembles gallery button, mode switcher, and camera switcher
    /// into a centered horizontal layout. The carousel visually extends
    /// beyond its layout bounds during expansion; SlideH slides the
    /// buttons outward in sync (reading from a shared atomic every frame).
    /// During recording, timelapse, quick-record, virtual-camera streaming,
    /// or a photo-timer countdown, the children are replaced with empty
    /// space of the same fixed height so the surrounding layout stays put.
    pub fn build_bottom_bar(&self) -> Element<'_, Message> {
        let bar_hidden = self.recording.is_recording()
            || self.quick_record.is_recording()
            || self.timelapse.is_active()
            || self.virtual_camera.is_streaming()
            || self.photo_timer_countdown.is_some();

        let inner: Element<'_, Message> = if bar_hidden {
            widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(BOTTOM_BAR_HEIGHT))
                .into()
        } else if self.mode.is_view_only() {
            // View mode: just the mode carousel — no gallery, no camera
            // switcher — but keep the same three-column shape so the carousel
            // sits at the same horizontal position as in the other modes.
            let spacing = cosmic::theme::spacing();
            let side = || -> Element<'_, Message> {
                widget::Space::new()
                    .width(Length::Fixed(
                        crate::constants::ui::PLACEHOLDER_BUTTON_WIDTH,
                    ))
                    .height(Length::Shrink)
                    .into()
            };
            three_col_row(
                side(),
                self.build_mode_switcher(),
                side(),
                [0, spacing.space_m],
            )
        } else {
            let spacing = cosmic::theme::spacing();
            let slide = std::sync::Arc::clone(&self.carousel_button_slide);
            // The carousel extends visually beyond its layout via render_bounds,
            // and SlideH slides the side buttons in sync with the expansion.
            three_col_row(
                SlideH::new(self.build_gallery_button(), slide.clone(), 1.0).into(),
                self.build_mode_switcher(),
                SlideH::new(self.build_camera_switcher(), slide, -1.0).into(),
                [0, spacing.space_m],
            )
        };

        widget::container(inner)
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
