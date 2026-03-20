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
const BOTTOM_BAR_HEIGHT: f32 = 74.0;

/// Duration of bottom bar fade animation in milliseconds
const FADE_DURATION_MS: f32 = 450.0;

impl AppModel {
    /// Start a bottom bar fade animation to the given target opacity.
    pub fn start_bottom_bar_fade(&mut self, target: f32) {
        let current = self.bottom_bar_current_opacity();
        self.bottom_bar_opacity_from = current;
        self.bottom_bar_opacity_target = target;
        self.bottom_bar_fade_start = Some(std::time::Instant::now());
    }

    /// Get the current interpolated bottom bar opacity.
    pub fn bottom_bar_current_opacity(&self) -> f32 {
        if let Some(start) = self.bottom_bar_fade_start {
            let elapsed = start.elapsed().as_secs_f32() * 1000.0;
            let t = (elapsed / FADE_DURATION_MS).min(1.0);
            let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic
            let opacity = self.bottom_bar_opacity_from
                + (self.bottom_bar_opacity_target - self.bottom_bar_opacity_from) * eased;
            opacity.clamp(0.0, 1.0)
        } else {
            self.bottom_bar_opacity_target
        }
    }

    /// Build the complete bottom bar widget
    ///
    /// Assembles gallery button, mode switcher, and camera switcher
    /// into a centered horizontal layout. The carousel visually extends
    /// beyond its layout bounds during expansion; SlideH slides the
    /// buttons outward in sync (reading from a shared atomic every frame).
    /// During recording, an overlay fades the bar out and blocks input.
    pub fn build_bottom_bar(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let slide = std::sync::Arc::clone(&self.carousel_button_slide);

        // Use Fill gaps so the row adapts to any screen width.
        // The carousel extends visually beyond its 150px layout via
        // render_bounds, and SlideH handles button positioning.
        let centered_row = widget::row()
            .push(SlideH::new(self.build_gallery_button(), slide.clone(), 1.0))
            .push(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Shrink),
            )
            .push(self.build_mode_switcher())
            .push(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Shrink),
            )
            .push(SlideH::new(self.build_camera_switcher(), slide, -1.0))
            .padding([0, spacing.space_m])
            .align_y(Alignment::Center);

        let bar = widget::container(centered_row)
            .width(Length::Fill)
            .height(Length::Fixed(BOTTOM_BAR_HEIGHT))
            .center_y(BOTTOM_BAR_HEIGHT)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            });

        let opacity = self.bottom_bar_current_opacity();

        let is_active = self.recording.is_recording()
            || self.quick_record.is_recording()
            || self.timelapse.is_active()
            || self.virtual_camera.is_streaming();

        // Always use the stack layout to avoid layout thrashing during animation.
        // The overlay is transparent when fully visible and blocks input when fading.
        let fade_alpha = (1.0 - opacity).max(0.0);
        let theme = cosmic::theme::active();
        let win_bg = theme.cosmic().bg_color();
        let blocks_input = is_active || opacity < 1.0;

        let overlay_container = widget::container(widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(BOTTOM_BAR_HEIGHT))
            .style(move |_theme| widget::container::Style {
                background: if fade_alpha > 0.001 {
                    Some(Background::Color(Color::from_rgba(
                        win_bg.red,
                        win_bg.green,
                        win_bg.blue,
                        fade_alpha,
                    )))
                } else {
                    None
                },
                ..Default::default()
            });

        let overlay: Element<'_, Message> = if blocks_input {
            widget::mouse_area(overlay_container)
                .on_press(Message::Noop)
                .on_release(Message::Noop)
                .into()
        } else {
            overlay_container.into()
        };

        cosmic::iced::widget::stack![bar, overlay]
            .width(Length::Fill)
            .height(Length::Fixed(BOTTOM_BAR_HEIGHT))
            .into()
    }
}
