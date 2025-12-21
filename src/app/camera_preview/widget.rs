// SPDX-License-Identifier: GPL-3.0-only

//! Camera preview widget implementation

use crate::app::state::{AppModel, Message};
use crate::app::video_widget::{self, VideoContentFit};
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Background, Length};
use cosmic::widget;
use std::sync::Arc;
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
            .style(|theme| widget::container::Style {
                background: Some(Background::Color(theme.cosmic().bg_color().into())),
                text_color: Some(theme.cosmic().on_bg_color().into()),
                ..Default::default()
            })
            .into();
        }

        // Build the main video preview (either current frame or placeholder)
        if let Some(frame) = &self.current_frame {
            static VIEW_FRAME_COUNT: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let count = VIEW_FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if count.is_multiple_of(30) {
                info!(
                    frame = count,
                    width = frame.width,
                    height = frame.height,
                    bytes = frame.data.len(),
                    "Rendering frame with video widget"
                );
            }

            // Check if 3D preview mode is enabled and we have a rendered point cloud
            if self.preview_3d.enabled {
                if let Some((width, height, rgba_data)) = &self.preview_3d.rendered_preview {
                    // Create a CameraFrame from point cloud data for video widget
                    let point_cloud_frame = Arc::new(crate::backends::camera::types::CameraFrame {
                        width: *width,
                        height: *height,
                        data: Arc::from(rgba_data.as_slice()),
                        depth_data: None,
                        depth_width: 0,
                        depth_height: 0,
                        format: crate::backends::camera::types::PixelFormat::RGBA,
                        stride: *width * 4,
                        captured_at: std::time::Instant::now(),
                        video_timestamp: None,
                    });

                    // Use video widget to display point cloud (video_id=2 for 3D preview)
                    // Use Cover mode to fill the entire preview area
                    let video_elem = video_widget::video_widget(
                        point_cloud_frame,
                        2, // Use separate video_id for 3D preview
                        VideoContentFit::Cover,
                        crate::app::state::FilterType::Standard,
                        0.0,
                        false, // No mirror for 3D
                        None,  // No crop
                        1.0,   // No zoom
                        false, // No scroll zoom
                    );

                    // Wrap in mouse_area for rotation control via drag and zoom via scroll
                    // Note: on_press doesn't provide position, so we start tracking on first move
                    let preview_with_mouse = widget::mouse_area(
                        widget::container(video_elem)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .align_x(cosmic::iced::alignment::Horizontal::Center)
                            .align_y(cosmic::iced::alignment::Vertical::Center),
                    )
                    .on_press(Message::Preview3DMousePressed(0.0, 0.0))
                    .on_move(|point: cosmic::iced::Point| {
                        Message::Preview3DMouseMoved(point.x, point.y)
                    })
                    .on_release(Message::Preview3DMouseReleased)
                    .on_scroll(|delta: cosmic::iced::mouse::ScrollDelta| {
                        // Convert scroll delta to zoom amount
                        let scroll_amount = match delta {
                            cosmic::iced::mouse::ScrollDelta::Lines { y, .. } => y * 100.0,
                            cosmic::iced::mouse::ScrollDelta::Pixels { y, .. } => y,
                        };
                        Message::Zoom3DPreview(scroll_amount)
                    });

                    return preview_with_mouse.into();
                } else {
                    // 3D mode enabled but no render yet - show loading
                    return widget::container(widget::text("Rendering 3D preview...").size(16))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(cosmic::iced::alignment::Horizontal::Center)
                        .align_y(cosmic::iced::alignment::Vertical::Center)
                        .style(|theme: &cosmic::Theme| widget::container::Style {
                            background: Some(Background::Color(theme.cosmic().bg_color().into())),
                            text_color: Some(theme.cosmic().on_bg_color().into()),
                            ..Default::default()
                        })
                        .into();
                }
            }

            // Use custom video widget with GPU primitive rendering
            // During transitions, use blur shader (video_id=1), otherwise normal shader (video_id=0)
            let should_blur = self.transition_state.should_blur();
            if should_blur && count.is_multiple_of(10) {
                info!("Applying blur to frame during transition");
            }
            let video_id = if should_blur { 1 } else { 0 };

            // Use Cover mode (fill/zoom) in theatre mode, Contain mode (letterbox) otherwise
            let content_fit = if self.theatre.enabled {
                VideoContentFit::Cover
            } else {
                VideoContentFit::Contain
            };

            // Apply filters in Photo, Virtual, and Scene modes (not in Video mode)
            let filter_mode = match self.mode {
                crate::app::state::CameraMode::Photo
                | crate::app::state::CameraMode::Virtual
                | crate::app::state::CameraMode::Scene => self.selected_filter,
                crate::app::state::CameraMode::Video => crate::app::state::FilterType::Standard,
            };
            // File sources should never be mirrored - they display content as-is
            // Use the flag that tracks if the current frame is actually from a file source
            let should_mirror = self.config.mirror_preview && !self.current_frame_is_file_source;

            // Calculate crop UV for aspect ratio (only in Photo mode, not in theatre mode)
            // Theatre mode always uses native resolution for full-screen display
            let crop_uv = match self.mode {
                crate::app::state::CameraMode::Photo if !self.theatre.enabled => {
                    self.photo_aspect_ratio.crop_uv(frame.width, frame.height)
                }
                _ => None,
            };

            // Apply zoom only in Photo mode
            let (zoom_level, scroll_zoom_enabled) = match self.mode {
                crate::app::state::CameraMode::Photo => (self.zoom_level, true),
                _ => (1.0, false),
            };

            let video_elem = video_widget::video_widget(
                frame.clone(),
                video_id,
                content_fit,
                filter_mode,
                0.0,
                should_mirror,
                crop_uv,
                zoom_level,
                scroll_zoom_enabled,
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
            if count.is_multiple_of(30) {
                info!(render_count = count, "No frame available in view");
            }

            // Themed canvas placeholder when no camera frame
            widget::container(widget::Space::new(Length::Fill, Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|theme: &cosmic::Theme| widget::container::Style {
                    background: Some(Background::Color(theme.cosmic().bg_color().into())),
                    ..Default::default()
                })
                .into()
        }
    }
}
