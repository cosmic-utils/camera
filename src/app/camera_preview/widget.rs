// SPDX-License-Identifier: GPL-3.0-only

//! Camera preview widget implementation

use crate::app::state::{AppModel, Message};
use crate::app::video_widget::{self, VideoContentFit};
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Background, Length};
use cosmic::widget;
use tracing::{debug, info};

impl AppModel {
    /// Whether the preview should be mirrored (front cameras only, not file sources)
    pub(crate) fn should_mirror_preview(&self) -> bool {
        let is_back = self
            .available_cameras
            .get(self.current_camera_index)
            .and_then(|c| c.camera_location.as_deref())
            == Some("back");
        self.config.mirror_preview && !self.current_frame_is_file_source && !is_back
    }

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
                debug!(
                    frame = count,
                    width = frame.width,
                    height = frame.height,
                    bytes = frame.data.len(),
                    "Rendering frame with video widget"
                );
            }

            // Use custom video widget with GPU primitive rendering
            // During transitions or HDR+ processing, use blur shader (video_id=1)
            let is_processing_hdr =
                self.burst_mode.stage == crate::app::state::BurstModeStage::Processing;
            let should_blur = self.transition_state.should_blur() || is_processing_hdr;
            if should_blur && count.is_multiple_of(10) {
                let reason = if is_processing_hdr {
                    "HDR+ processing"
                } else {
                    "transition"
                };
                info!("Applying blur to frame during {}", reason);
            }
            let video_id = if should_blur {
                crate::app::video_primitive::VIDEO_ID_BLUR
            } else {
                crate::app::video_primitive::VIDEO_ID_NORMAL
            };

            // Always use Cover mode (fill/zoom) — preview fills available space
            let content_fit = VideoContentFit::Cover;

            let filter_mode = self.selected_filter;

            // During blur transitions, use the rotation and mirror state captured
            // at transition start (from the old camera) since the blur frame is
            // from the old camera.
            let (sensor_rotation, should_mirror) = if should_blur {
                (self.blur_frame_rotation, self.blur_frame_mirror)
            } else {
                (self.current_frame_rotation, self.should_mirror_preview())
            };
            let rotation = sensor_rotation.gpu_rotation_code();

            // Preview always shows the full sensor image (no crop).
            // Aspect ratio only affects photo capture, not the live preview.
            let crop_uv = None;

            // Apply zoom only in Photo mode
            let (zoom_level, scroll_zoom_enabled) = match self.mode {
                crate::app::state::CameraMode::Photo => (self.zoom_level, true),
                _ => (1.0, false),
            };

            let video_elem = video_widget::video_widget(
                frame.clone(),
                video_widget::VideoWidgetConfig {
                    video_id,
                    content_fit,
                    filter_type: filter_mode,
                    corner_radius: 0.0,
                    mirror_horizontal: should_mirror,
                    rotation,
                    crop_uv,
                    zoom_level,
                    scroll_zoom_enabled,
                },
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
            widget::container(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
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
