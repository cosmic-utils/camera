// SPDX-License-Identifier: GPL-3.0-only

//! Composition guide overlay module
//!
//! Renders composition guide lines (Rule of Thirds, Phi Grid, etc.)
//! on top of the camera preview using a canvas widget.

mod widget;

use crate::app::state::{AppModel, CameraMode, Message};
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
    /// Passes the live state needed to compute the visible-video rectangle
    /// at draw time so the guide tracks fit/fill state, the photo aspect-
    /// ratio crop, and the bottom-bar scrim height (which differs by mode
    /// and animates across mode switches).
    pub fn build_composition_overlay(&self) -> Element<'_, Message> {
        if self.config.composition_guide == CompositionGuide::None {
            return empty_overlay();
        }

        let Some(frame) = &self.current_frame else {
            return empty_overlay();
        };

        let rotation = self.current_frame_rotation;
        let (rotated_w, rotated_h) = if rotation.swaps_dimensions() {
            (frame.height as f32, frame.width as f32)
        } else {
            (frame.width as f32, frame.height as f32)
        };
        if rotated_w < 1.0 || rotated_h < 1.0 {
            return empty_overlay();
        }

        // Aspect-ratio crop applies in Photo mode only; non-Native ratios
        // produce a sub-rect in Cover and a different letterbox in Contain.
        // Use the *display*-oriented ratio so the guide aligns with the
        // rotated preview on portrait windows.
        let aspect_crop_ratio =
            if self.mode == CameraMode::Photo && !self.current_frame_is_file_source {
                self.photo_aspect_ratio
                    .display_ratio(self.screen_is_portrait())
            } else {
                None
            };

        widget::composition_canvas(
            self.config.composition_guide,
            rotated_w,
            rotated_h,
            aspect_crop_ratio,
            self.cover_blend(),
            // Pass the *animated* top/bottom heights so the guide tracks
            // the scrim through Photo↔View transitions; in View the bars
            // settle at 0 and the guide aligns with the full window.
            self.top_ui_height(),
            self.bottom_ui_height(),
        )
    }
}
