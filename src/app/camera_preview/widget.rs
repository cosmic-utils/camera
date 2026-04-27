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

            // Always use Cover layout — the shader interpolates between Cover and
            // Contain via the cover_blend value for smooth animated transitions.
            let cover_blend = self.cover_blend();
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

            // Always pass the target Contain crop for Photo mode.  The shader
            // interpolates the crop region toward the full texture as `cover_blend`
            // approaches 1.0, so Cover mode and mid-animation frames degenerate to
            // the uncropped view without a discrete snap.
            let crop_uv = match self.mode {
                crate::app::state::CameraMode::Photo if !self.current_frame_is_file_source => {
                    self.photo_aspect_ratio.crop_uv(frame.width, frame.height)
                }
                _ => None,
            };

            // Read the animated zoom in every mode so a Photo→non-Photo
            // mode switch eases the zoom back to 1× instead of snapping.
            // Settled `zoom_level` is reset to 1.0 by `handle_set_mode`,
            // so this is only ever != 1.0 mid-animation. Scroll/pinch zoom
            // are gated on modes that support manual zoom (Photo, View).
            let zoom_level = self.current_zoom_level();
            let scroll_zoom_enabled = self.mode.supports_fit_and_zoom();

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
                    cover_blend: Some(cover_blend),
                    bar_top_px: self.top_ui_height(),
                    bar_bottom_px: self.bottom_ui_height(),
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
