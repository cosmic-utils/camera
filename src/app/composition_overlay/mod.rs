// SPDX-License-Identifier: GPL-3.0-only

//! Composition guide overlay module
//!
//! Renders composition guide lines (Rule of Thirds, Phi Grid, etc.)
//! on top of the camera preview using a canvas widget.

mod widget;

use crate::app::state::{AppModel, Message};
use crate::app::video_widget::VideoContentFit;
use crate::config::CompositionGuide;
use cosmic::Element;
use cosmic::iced::Length;

/// Full-size invisible spacer (used when no overlay is needed).
fn empty_overlay<'a>() -> Element<'a, Message> {
    cosmic::widget::Space::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

impl AppModel {
    /// Build the composition guide overlay element.
    ///
    /// Computes the same effective frame dimensions as the video widget
    /// (accounting for sensor rotation and aspect ratio crop) so that
    /// guide lines align with the visible video content.
    pub fn build_composition_overlay(&self) -> Element<'_, Message> {
        if self.config.composition_guide == CompositionGuide::None {
            return empty_overlay();
        }

        let Some(frame) = &self.current_frame else {
            return empty_overlay();
        };

        let content_fit = VideoContentFit::Cover;

        // Match the video widget's effective dimensions:
        // 1. Apply sensor rotation (swap dimensions for 90°/270°)
        let rotation = self.current_frame_rotation;
        let (ew, eh) = if rotation.swaps_dimensions() {
            (frame.height as f32, frame.width as f32)
        } else {
            (frame.width as f32, frame.height as f32)
        };

        // Preview always shows full sensor image (no crop)
        let (fw, fh) = (ew.round(), eh.round());

        if fw < 1.0 || fh < 1.0 {
            return empty_overlay();
        }

        widget::composition_canvas(
            self.config.composition_guide,
            fw as u32,
            fh as u32,
            content_fit,
        )
    }
}
