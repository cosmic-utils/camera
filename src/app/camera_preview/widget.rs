// SPDX-License-Identifier: GPL-3.0-only

//! Camera preview widget implementation

use crate::app::state::{AppModel, Message};
use crate::app::video_widget::{self, VideoContentFit};
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Background, Color, Length};
use cosmic::widget;
use tracing::info;

impl AppModel {
    /// Build the camera preview widget
    ///
    /// Uses custom video widget with handle caching for optimized rendering.
    /// Shows a loading indicator when cameras are initializing.
    /// Shows a black placeholder when no camera frame is available.
    /// Shows a blurred last frame during camera transitions.
    pub fn build_camera_preview(&self) -> Element<'_, Message> {
        // Show loading indicator if cameras aren't initialized yet
        if self.available_cameras.is_empty() {
            return widget::container(
                widget::column()
                    .push(widget::text(fl!("initializing-camera")).size(20))
                    .spacing(10)
                    .align_x(cosmic::iced::alignment::Horizontal::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::BLACK)),
                text_color: Some(Color::WHITE),
                ..Default::default()
            })
            .into();
        }

        // Build the main video preview (either current frame or placeholder)
        if let Some(frame) = &self.current_frame {
            static VIEW_FRAME_COUNT: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let count = VIEW_FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if count % 30 == 0 {
                info!(
                    frame = count,
                    width = frame.width,
                    height = frame.height,
                    bytes = frame.data.len(),
                    "Rendering frame with video widget"
                );
            }

            // Use custom video widget with GPU primitive rendering
            // During transitions, use blur shader (video_id=1), otherwise normal shader (video_id=0)
            let should_blur = self.transition_state.should_blur();
            if should_blur && count % 10 == 0 {
                info!("Applying blur to frame during transition");
            }
            let video_id = if should_blur { 1 } else { 0 };

            // Use Cover mode (fill/zoom) in theatre mode, Contain mode (letterbox) otherwise
            let content_fit = if self.theatre.enabled {
                VideoContentFit::Cover
            } else {
                VideoContentFit::Contain
            };

            // Apply filters in Photo and Virtual modes (not in Video mode)
            let filter_mode = match self.mode {
                crate::app::state::CameraMode::Photo | crate::app::state::CameraMode::Virtual => {
                    self.selected_filter
                }
                crate::app::state::CameraMode::Video => crate::app::state::FilterType::Standard,
            };
            // File sources should never be mirrored - they display content as-is
            // Use the flag that tracks if the current frame is actually from a file source
            let should_mirror = self.config.mirror_preview && !self.current_frame_is_file_source;

            let video_elem = video_widget::video_widget(
                frame.clone(),
                video_id,
                content_fit,
                filter_mode,
                0.0,
                should_mirror,
            );

            widget::container(video_elem)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center)
                .align_y(cosmic::iced::alignment::Vertical::Center)
                .into()
        } else {
            static NO_FRAME_COUNT: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let count = NO_FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if count % 30 == 0 {
                info!(render_count = count, "No frame available in view");
            }

            // Black canvas placeholder when no camera frame
            widget::container(widget::Space::new(Length::Fill, Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_theme| widget::container::Style {
                    background: Some(Background::Color(Color::BLACK)),
                    ..Default::default()
                })
                .into()
        }
    }
}
