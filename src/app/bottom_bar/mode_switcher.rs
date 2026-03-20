// SPDX-License-Identifier: GPL-3.0-only

//! Mode switcher — builds a ModeCarousel widget for mode selection.

use crate::app::bottom_bar::mode_carousel::ModeCarousel;
use crate::app::state::{AppModel, Message};
use cosmic::Element;

impl AppModel {
    /// Build the mode switcher widget using the custom ModeCarousel.
    pub fn build_mode_switcher(&self) -> Element<'_, Message> {
        // Allow mode switching during blur transitions (camera restart) —
        // only disable during recording, streaming, or timelapse.
        let is_disabled = self.recording.is_recording()
            || self.virtual_camera.is_streaming()
            || self.timelapse.is_active();

        let modes = self.available_modes();

        ModeCarousel::new(
            modes,
            self.mode,
            Message::SetMode,
            is_disabled,
            std::sync::Arc::clone(&self.carousel_button_slide),
        )
        .into()
    }
}
